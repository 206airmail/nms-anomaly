//! Recording a game session, from the app's side.
//!
//! One background thread, for the whole life of the app: it waits for `NMS.exe`
//! to appear, records everything the hook sends while it runs, finishes the
//! session off when the game exits, and goes back to waiting. Nothing about it
//! is tied to how the game was started, so a run launched from Steam, from a
//! desktop shortcut or from this program's own Play button is recorded the same.
//!
//! **Why a thread and not a timer.** Both halves of this are blocking waits
//! Windows already does well -- waiting for a pipe client, and reading it until
//! it ends. Polling either one would either burn CPU or miss the first seconds
//! of a session, which is exactly when the game opens every mod file it is going
//! to open.
//!
//! **The pipe is opened when the game is seen, not at startup.** The hook only
//! talks while the game runs, and holding the single-instance pipe open for the
//! whole life of the app would stop any other copy of this program -- or the
//! standalone logger someone still has -- from ever recording, including while
//! we are not interested.
//!
//! **Nothing here can make the game do anything.** The hook writes, this reads.
//! The one thing it acts on is its own log folder.
//!
//! The screen is kept up to date two ways: [`State`], emitted whenever it
//! changes and readable on demand, and batches of events -- at most a few times
//! a second, because the game opens a couple of hundred files in its first
//! second and one message per line would drown the webview.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use crate::engine::sessionlog::{self, Brief, Counts, Live, Recorder};
use crate::engine::{clock, gameproc, hook, machine, pipe, savewatch, settings};

/// How often the game is looked for while it is not running.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How long to wait for the game to actually exit after the pipe closes.
///
/// The hook's pipe closes on `DLL_PROCESS_DETACH`, which is early in a tidy
/// shutdown -- the process can take a good few seconds after that. Waiting is
/// what makes an exit code, and therefore "did it crash", available at all.
const EXIT_PATIENCE: Duration = Duration::from_secs(20);

/// Windows Error Reporting files its event *after* the process has gone, so a
/// crash is worth this pause before the summary is written.
const WER_PATIENCE: Duration = Duration::from_secs(3);

/// How often saves are copied while a game runs, when the hook is not there to
/// say the moment one was written.
const BACKUP_EVERY: Duration = Duration::from_secs(20);

/// Longest a batch of events waits before the screen gets it.
const FLUSH_EVERY: Duration = Duration::from_millis(250);

/// Events kept for a screen opened in the middle of a session.
const TAIL: usize = 600;

/// Sessions kept on disk when the user has not chosen a number.
pub const KEEP_DEFAULT: usize = 25;

/// What the recorder is doing, as the screen shows it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    /// the watching thread is alive
    pub watching: bool,
    pub game_running: bool,
    pub pid: u32,
    /// a session is open: the hook is connected and lines are arriving
    pub recording: bool,
    /// when the open session started, in unix milliseconds
    pub since_ms: i64,
    pub counts: Counts,
    /// the log being written, while one is
    pub log: Option<String>,
    /// true when the hook DLL is in place, so a run would be recorded
    pub hook_ready: bool,
    /// why nothing is being recorded, when nothing is
    pub note: Option<String>,
    /// the session that finished last, for the screen to show straight away
    pub last: Option<Brief>,
}

/// The recorder, as the rest of the app holds it.
#[derive(Clone)]
pub struct Watcher {
    inner: Arc<Inner>,
}

struct Inner {
    stop: AtomicBool,
    started: AtomicBool,
    state: Mutex<State>,
    tail: Mutex<std::collections::VecDeque<Live>>,
}

impl Watcher {
    pub fn new() -> Watcher {
        Watcher {
            inner: Arc::new(Inner {
                stop: AtomicBool::new(false),
                started: AtomicBool::new(false),
                state: Mutex::new(State::default()),
                tail: Mutex::new(std::collections::VecDeque::new()),
            }),
        }
    }

    pub fn state(&self) -> State {
        self.inner
            .state
            .lock()
            .map(|held| held.clone())
            .unwrap_or_default()
    }

    /// The events of the open session, oldest first.
    pub fn tail(&self) -> Vec<Live> {
        self.inner
            .tail
            .lock()
            .map(|held| held.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Stop watching. The thread ends after the session it is recording.
    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        // A thread blocked on the pipe is waiting on Windows, not on a flag.
        pipe::nudge();
    }

    /// Start watching, once. Safe to call again; the second call does nothing.
    pub fn start(&self, app: tauri::AppHandle, sessions: PathBuf) {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("nms-session-recorder".into())
            .spawn(move || run(inner, app, sessions))
            .map(|_| ())
            .unwrap_or_else(|err| {
                // A machine that cannot spawn a thread has bigger problems, but
                // the screen should still say why it is not recording.
                let _ = err;
            });
    }
}

impl Default for Watcher {
    fn default() -> Self {
        Watcher::new()
    }
}

/// Where sessions are kept.
pub fn sessions_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    crate::app_data(app).map(|dir| dir.join("sessions"))
}

/// Where copies of the save are kept.
pub fn backups_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    crate::app_data(app).map(|dir| dir.join("save-backups"))
}

/// One backup pass at a time, however many things ask for one.
static COPYING: AtomicBool = AtomicBool::new(false);

/// Take copies of any save that has changed, off the calling thread.
///
/// Off the thread because the caller is usually the one reading the pipe, and
/// copying fifty megabytes there would stall the live view -- the hook buffers
/// what it cannot send, so nothing is lost, but the screen would go quiet for
/// half a second every time the game saved.
pub fn keep_saves(app: &tauri::AppHandle) {
    let chosen = crate::settings_now();
    if !chosen.backup_saves {
        return;
    }
    let Ok(root) = backups_dir(app) else {
        return;
    };
    if COPYING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let keep = chosen.keep_save_backups;
    let started = std::thread::Builder::new()
        .name("nms-save-backup".into())
        .spawn(move || {
            let dirs = savewatch::save_dirs();
            if !dirs.is_empty() {
                let passed = savewatch::back_up(&dirs, &root, keep);
                if !passed.taken.is_empty() || !passed.problems.is_empty() {
                    let _ = app.emit("saves-kept", &passed);
                }
            }
            COPYING.store(false, Ordering::SeqCst);
        });
    if started.is_err() {
        COPYING.store(false, Ordering::SeqCst);
    }
}

/// The folder holding `NMS.exe`, which is where the hook has to live.
///
/// Read off the resolved executable rather than assembled from the game folder:
/// the GOG and Game Pass layouts do not both put it in `Binaries`, and a user who
/// has pointed at their own executable in Settings has said where it is.
pub fn bin_dir(found: &settings::Resolved) -> Option<PathBuf> {
    found
        .game_exe
        .as_path()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .or_else(|| found.game_root.as_path().map(|root| root.join("Binaries")))
}

/// The whole loop: wait, record, finish, wait again.
fn run(inner: Arc<Inner>, app: tauri::AppHandle, sessions: PathBuf) {
    publish(&inner, &app, |state| {
        state.watching = true;
    });
    // A baseline before anything else happens: whatever the saves look like when
    // the app opens is worth a copy, because it is the last state before whatever
    // happens next.
    keep_saves(&app);

    while !inner.stop.load(Ordering::SeqCst) {
        let chosen = crate::settings_now();
        // Read every pass rather than once at startup, so turning recording off
        // in Settings takes effect without restarting the app -- and so that
        // turning it back on does not need a restart either.
        if !chosen.record_sessions {
            publish(&inner, &app, |state| {
                state.watching = false;
                state.recording = false;
                state.log = None;
                state.note = Some("Recording is switched off in Settings".into());
            });
            std::thread::sleep(LOOK_EVERY);
            continue;
        }
        let found = settings::resolve(&chosen);
        let bin = bin_dir(&found);
        let hook_state = hook::state(bin.as_deref());

        let Some(pid) = gameproc::find() else {
            publish(&inner, &app, |state| {
                state.watching = true;
                state.game_running = false;
                state.pid = 0;
                state.recording = false;
                state.hook_ready = hook_state.recording();
                state.log = None;
                state.note = if hook_state.recording() {
                    None
                } else {
                    Some(waiting_note(&hook_state))
                };
            });
            std::thread::sleep(LOOK_EVERY);
            continue;
        };

        // The game is up. Hold it open now, so its exit code survives its exit.
        let running = gameproc::open(pid);
        publish(&inner, &app, |state| {
            state.game_running = true;
            state.pid = pid;
            state.hook_ready = hook_state.recording();
            state.note = if hook_state.recording() {
                None
            } else {
                Some(format!(
                    "The game is running, but nothing is being recorded: {}",
                    waiting_note(&hook_state)
                ))
            };
        });

        match record(&inner, &app, &sessions, &found, running.as_ref()) {
            Ok(()) => {}
            Err(problem) => {
                publish(&inner, &app, |state| {
                    state.recording = false;
                    state.note = Some(problem.clone());
                });
                // Do not spin on a pipe that cannot be opened -- something else
                // owns it, and that will not change in a hurry.
                std::thread::sleep(Duration::from_secs(5));
            }
        }

        // Wait out the rest of this run before looking again, so a game that
        // outlives a failed recording does not start a new attempt every second.
        while !inner.stop.load(Ordering::SeqCst) && gameproc::find() == Some(pid) {
            std::thread::sleep(LOOK_EVERY);
        }
    }

    publish(&inner, &app, |state| {
        state.watching = false;
        state.recording = false;
    });
}

/// Why a run would not be recorded as things stand.
fn waiting_note(state: &hook::State) -> String {
    if let Some(said) = &state.foreign {
        return format!("{said}. Installing the recorder would replace it.");
    }
    if !state.installed {
        return "the recorder is not installed in the game yet".to_string();
    }
    if let Some(said) = &state.blocked {
        return said.clone();
    }
    "the recorder is not installed in the game yet".to_string()
}

/// Record one connection from the hook, start to finish.
fn record(
    inner: &Arc<Inner>,
    app: &tauri::AppHandle,
    sessions: &Path,
    found: &settings::Resolved,
    running: Option<&gameproc::Running>,
) -> Result<(), String> {
    let server = pipe::Server::open()?;

    // Once the game is gone there will never be a client, so a waiting thread
    // has to be woken. Same for the app closing.
    let alive = Arc::new(AtomicBool::new(true));
    let watchdog = {
        let inner = inner.clone();
        let alive = alive.clone();
        let pid = running.map(|r| r.pid);
        let watching = app.clone();
        std::thread::Builder::new()
            .name("nms-session-watchdog".into())
            .spawn(move || {
                let mut next_backup = Instant::now() + BACKUP_EVERY;
                while alive.load(Ordering::SeqCst) && !inner.stop.load(Ordering::SeqCst) {
                    if gameproc::find() != pid {
                        break;
                    }
                    // The hook reports a save the moment it is finished, which is
                    // the better trigger -- but it is only installed if the user
                    // installed it, and the copies matter either way.
                    if Instant::now() >= next_backup {
                        keep_saves(&watching);
                        next_backup = Instant::now() + BACKUP_EVERY;
                    }
                    std::thread::sleep(LOOK_EVERY);
                }
                pipe::nudge();
            })
            .ok()
    };
    // Whatever happens below, the watchdog must be let go of.
    let _done = Guard(alive.clone(), watchdog);

    server.wait_for_client()?;
    if inner.stop.load(Ordering::SeqCst) {
        return Ok(());
    }

    // What the last recorded session ran on, before this one's log is written --
    // otherwise the newest brief would be our own and every session would report
    // that nothing had changed.
    let previously = sessionlog::list(sessions)
        .into_iter()
        .find_map(|brief| brief.machine);
    let game_root = found
        .game_root
        .as_path()
        .unwrap_or_else(|| PathBuf::from("."));
    let save_dirs = savewatch::save_dirs();
    let now = machine::Machine::now(Some(&game_root), save_dirs.first().map(|dir| dir.as_path()));
    let changed = previously
        .as_ref()
        .map(|before| machine::changes(before, &now))
        .unwrap_or_default();

    let about = sessionlog::About {
        game_root,
        bin_dir: bin_dir(found).unwrap_or_else(|| PathBuf::from(".")),
        mods_dir: found.mods_dir.as_path().unwrap_or_else(|| PathBuf::from(".")),
        app_version: app.package_info().version.to_string(),
        machine: Some(now),
        changed,
    };
    // Read before the first event lands: this is what the library looked like
    // when the game started, which is what the file-usage check must compare
    // against. A mod installed mid-session is not a mod the game skipped.
    let mods = sessionlog::inventory(&about.mods_dir, &about.game_root);

    let mut recorder: Option<Recorder> = None;
    let mut refused: Option<String> = None;
    let mut batch: Vec<Live> = Vec::new();
    let mut sent_at = Instant::now();

    let ended = server.read_lines(
        |line| {
            if recorder.is_none() {
                if refused.is_some() {
                    return;
                }
                // The first line is the hook's greeting, which carries the pid
                // and version the header wants. Anything else is still an
                // event, and is recorded once the log is open.
                let hello = match sessionlog::parse(line) {
                    sessionlog::Line::Hello(hello) => Some(hello),
                    _ => None,
                };
                let opening = hello.is_some();
                match Recorder::start(sessions, about.clone(), mods.clone(), hello) {
                    Ok(started) => {
                        let log = started.log_path().display().to_string();
                        let since = started.started_ms();
                        recorder = Some(started);
                        if let Ok(mut held) = inner.tail.lock() {
                            held.clear();
                        }
                        publish(inner, app, |state| {
                            state.recording = true;
                            state.since_ms = since;
                            state.counts = Counts::default();
                            state.log = Some(log.clone());
                            state.note = None;
                        });
                    }
                    Err(err) => {
                        refused = Some(err);
                        return;
                    }
                }
                if opening {
                    return;
                }
            }
            let Some(open) = recorder.as_mut() else {
                return;
            };
            if let Some(live) = open.take(line) {
                // A save the game has just finished writing is the one moment a
                // copy of it is both complete and current. Prompt rather than
                // periodic -- the periodic pass below is the fallback for a
                // session with no hook in it at all.
                if live.cat == "save" && live.lvl < 2 {
                    keep_saves(app);
                }
                remember(inner, &live);
                batch.push(live);
            }
            if sent_at.elapsed() >= FLUSH_EVERY {
                flush(inner, app, &mut batch, open.counts().clone());
                sent_at = Instant::now();
            }
        },
        || !inner.stop.load(Ordering::SeqCst),
    );

    // Whatever is left over goes out before the summary, so the screen is not
    // showing a session that ends mid-sentence.
    if let Some(open) = recorder.as_ref() {
        flush(inner, app, &mut batch, open.counts().clone());
    }
    server.disconnect();

    if let Some(problem) = refused {
        return Err(problem);
    }
    let Some(open) = recorder else {
        // A connection that sent nothing: our own nudge, almost always, which
        // means the game went away before the hook ever got through.
        return Ok(());
    };

    let stopped_early = matches!(ended, Ok(pipe::Ended::Asked));
    let exit_code = running.and_then(|game| {
        game.wait_for_exit(if stopped_early {
            Duration::from_millis(500)
        } else {
            EXIT_PATIENCE
        })
    });
    if exit_code.map(gameproc::is_crash_code).unwrap_or(false) {
        std::thread::sleep(WER_PATIENCE);
    }

    // The last save of the session is the one most worth having, and the game may
    // have written it in the seconds after the pipe closed.
    keep_saves(app);
    let brief = open.finish(exit_code, stopped_early);
    let keep = crate::settings_now().keep_sessions.max(1) as usize;
    sessionlog::prune(sessions, keep);

    publish(inner, app, |state| {
        state.recording = false;
        state.log = None;
        state.counts = brief.counts.clone();
        state.last = Some(brief.clone());
        state.note = None;
    });
    let _ = app.emit("session-ended", &brief);
    Ok(())
}

/// Ends the watchdog thread, whichever way the recording ended.
struct Guard(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
        // Waited for rather than detached, even though it costs up to one of
        // its two-second naps. A watchdog still alive from the *previous*
        // session would nudge the pipe the next one is waiting on, and that
        // wait is how a game launched a minute later gets recorded at all.
        if let Some(thread) = self.1.take() {
            let _ = thread.join();
        }
    }
}

fn remember(inner: &Arc<Inner>, live: &Live) {
    if let Ok(mut held) = inner.tail.lock() {
        held.push_back(live.clone());
        while held.len() > TAIL {
            held.pop_front();
        }
    }
}

fn flush(inner: &Arc<Inner>, app: &tauri::AppHandle, batch: &mut Vec<Live>, counts: Counts) {
    if let Ok(mut state) = inner.state.lock() {
        state.counts = counts;
    }
    if batch.is_empty() {
        return;
    }
    let _ = app.emit("session-events", &*batch);
    batch.clear();
}

/// Change the state and tell the screen, in that order.
fn publish(inner: &Arc<Inner>, app: &tauri::AppHandle, change: impl FnOnce(&mut State)) {
    let now = {
        let Ok(mut state) = inner.state.lock() else {
            return;
        };
        change(&mut state);
        state.clone()
    };
    let _ = app.emit("watch-state", &now);
}

/// A reading of the clock the UI can show while a session is open.
pub fn seconds_since(ms: i64) -> u64 {
    ((clock::now_ms() - ms).max(0) / 1000) as u64
}
