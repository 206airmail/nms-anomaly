//! One run of the game, written down.
//!
//! The hook inside `NMS.exe` sends a JSON object per line down a pipe (see
//! [`super::pipe`]); this turns that stream into two things: a text log a person
//! can read or attach to a bug report, and a [`Brief`] -- the one-line verdict
//! and the counts the Sessions screen lists without opening anything.
//!
//! Ported from the C# host of the standalone NMS Logger, whose behaviour was
//! settled against real sessions. The parts that look fussy are the parts that
//! were wrong first:
//!
//! **The game says everything twice.** Every line it logs goes both to
//! `FullLog.txt` (which the hook captures as `gamelog`) and to
//! `OutputDebugString` (captured as `debugout`, prefixed with the module name).
//! Without [`Echo`] the log reads as if every complaint happened twice, which
//! also doubles every count on the screen.
//!
//! **A mod named in a crash report did not cause the crash.** The report lists
//! the last 48 files the game opened as context, and attributing those to their
//! mods put a blameless mod at the top of the "reported trouble" list. So
//! attribution skips the `crash`, `module` and `hook` categories.
//!
//! **"The game never opened this file" is not one finding.** It means three
//! different things, and only one of them is definite: a file whose other-format
//! twin *was* opened is genuinely dead weight, while the rest may simply be
//! situational content the session never reached. They are reported apart,
//! because a list that mixes them teaches the reader to ignore all of it.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::crashinfo::{self, Aftermath};
use super::{clock, gameproc, hook, hostenv, machine};

/// How the levels the hook sends are spelled in the log.
const LEVELS: [&str; 5] = ["DEBUG", "INFO ", "WARN ", "ERROR", "FATAL"];

/// Width of the `21:12:46.123 WARN  [modfile  ] ` prefix, so a multi-line
/// message lines up under its own first line instead of under the margin.
const PREFIX: usize = 31;

/// Lines of the game's own log kept for spotting the echo of each one.
const ECHO_MEMORY: usize = 256;

/// How many example lines are kept per mod. Enough to recognise the problem,
/// few enough that a mod failing on every asset does not fill the summary.
const SAMPLES: usize = 5;

/// Extensions that are documentation, not something the game loads.
///
/// A mod made of nothing but these cannot do anything, which is a finding --
/// usually an AMUMSS script that was never compiled.
const NOT_GAME_FILES: [&str; 24] = [
    ".lua", ".txt", ".md", ".json", ".yml", ".yaml", ".ini", ".url", ".pdf", ".doc", ".docx",
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".zip", ".7z", ".rar", ".html", ".htm", ".bak",
    ".log", ".exml.bak",
];

/// The three spellings of one asset. A mod shipping two of them wastes one.
const DATA_EXTS: [&str; 3] = [".MBIN", ".MXML", ".EXML"];

// ---------------------------------------------------------------------------
// what arrives on the pipe
// ---------------------------------------------------------------------------

/// The hook's opening line, resent on every reconnect.
#[derive(Debug, Clone)]
pub struct Hello {
    pub pid: u32,
    pub hook_version: String,
    pub exe: String,
    /// events the hook had to drop before anything was listening
    pub backlog_dropped: u64,
}

/// One thing the hook saw.
#[derive(Debug, Clone)]
pub struct Event {
    /// unix milliseconds, from inside the game
    pub ts: i64,
    /// 0 debug, 1 info, 2 warn, 3 error, 4 fatal
    pub lvl: u8,
    pub tid: u32,
    pub cat: String,
    /// may be several lines: crash reports and module lists are
    pub msg: String,
}

/// A line off the pipe, whatever it turned out to be.
#[derive(Debug, Clone)]
pub enum Line {
    Hello(Hello),
    Event(Event),
    /// something we could not read. Kept rather than dropped: a line we cannot
    /// parse is a bug in one of the two halves, and silence hides it.
    Junk(String),
}

/// Read one line of the pipe protocol.
///
/// The `type` field marks a control line; everything else is an event. Parsed
/// field by field rather than into a struct so that a hook one version ahead,
/// sending a field this build has never heard of, still logs its events.
pub fn parse(text: &str) -> Line {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Line::Junk(String::new());
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Line::Junk(trimmed.to_string());
    };
    let string = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    if value.get("type").and_then(|v| v.as_str()) == Some("hello") {
        return Line::Hello(Hello {
            pid: value.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            hook_version: {
                let said = string("hookVersion");
                if said.is_empty() {
                    "?".to_string()
                } else {
                    said
                }
            },
            exe: string("exe"),
            backlog_dropped: value
                .get("backlogDropped")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        });
    }
    if value.get("type").is_some() {
        // A control line from a newer hook. Not an event, not a fault.
        return Line::Junk(trimmed.to_string());
    }

    let msg = string("msg");
    if msg.is_empty() && value.get("ts").is_none() {
        return Line::Junk(trimmed.to_string());
    }
    Line::Event(Event {
        ts: value
            .get("ts")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(clock::now_ms),
        lvl: value.get("lvl").and_then(|v| v.as_u64()).unwrap_or(1).min(4) as u8,
        tid: value.get("tid").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        cat: {
            let said = string("cat");
            if said.is_empty() {
                "event".to_string()
            } else {
                said
            }
        },
        msg,
    })
}

// ---------------------------------------------------------------------------
// the echo
// ---------------------------------------------------------------------------

/// Drops the second copy of every line the game logs twice.
///
/// Neither source is preferred, because which one arrives first is not stable:
/// whichever copy of a line turns up first is kept and the other is dropped when
/// it follows. Keyed by source so a line only ever cancels the *other* source's
/// copy of itself -- a game that really logs the same sentence twice still shows
/// it twice.
#[derive(Default)]
pub struct Echo {
    order: VecDeque<String>,
    waiting: HashSet<String>,
}

/// What `OutputDebugString` capture puts in front of the game's own text.
const DEBUG_PREFIX: &str = "[NMS.exe] ";

impl Echo {
    /// True when this event is the echo of one already recorded.
    pub fn is_echo(&mut self, cat: &str, msg: &str) -> bool {
        let body = match cat {
            "gamelog" => msg,
            "debugout" => match msg.strip_prefix(DEBUG_PREFIX) {
                Some(rest) => rest,
                None => return false,
            },
            _ => return false,
        };

        let mine = format!("{cat}\u{1}{body}");
        let theirs = format!(
            "{}\u{1}{body}",
            if cat == "gamelog" { "debugout" } else { "gamelog" }
        );
        if self.waiting.remove(&theirs) {
            return true;
        }
        if self.waiting.insert(mine.clone()) {
            self.order.push_back(mine);
        }
        while self.order.len() > ECHO_MEMORY {
            if let Some(old) = self.order.pop_front() {
                self.waiting.remove(&old);
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// what the library looked like when the session started
// ---------------------------------------------------------------------------

/// One mod folder as the game would see it, for the file-usage check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModFiles {
    pub folder: String,
    pub enabled: bool,
    /// paths relative to the mod folder, in the shape the hook reports them
    pub files: Vec<String>,
}

/// Every mod folder in the game's MODS directory, with its loadable files.
///
/// This is not the conflict scan -- that is the Actions tab, and it costs
/// seconds. This is a directory walk, because a session log has to be opened at
/// the moment the game starts and the reader is already playing.
pub fn inventory(mods_dir: &Path, game_root: &Path) -> Vec<ModFiles> {
    let settings = hostenv::read_mod_settings(game_root);
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return Vec::new();
    };

    let mut found: Vec<ModFiles> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let folder = entry.file_name().to_string_lossy().to_string();
            let mut files = Vec::new();
            walk(&entry.path(), "", &mut files);
            files.sort();
            ModFiles {
                enabled: !settings.is_disabled(&folder),
                folder,
                files,
            }
        })
        .collect();
    found.sort_by_key(|entry| entry.folder.to_lowercase());
    found
}

fn walk(dir: &Path, prefix: &str, into: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}\\{name}")
        };
        if entry.path().is_dir() {
            walk(&entry.path(), &rel, into);
        } else if is_game_file(&name) {
            into.push(rel);
        }
    }
}

/// True when the game could load this file, as opposed to a readme or a script.
fn is_game_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    // A manager's own bookkeeping, not the mod's content.
    if lower.starts_with("__folder_managed_by") || lower == hostenv::VORTEX_MANIFEST {
        return false;
    }
    !NOT_GAME_FILES.iter().any(|ext| lower.ends_with(ext))
}

// ---------------------------------------------------------------------------
// what a session amounted to
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Counts {
    pub fatal: u64,
    pub error: u64,
    pub warn: u64,
    pub info: u64,
    pub debug: u64,
}

impl Counts {
    fn add(&mut self, lvl: u8) {
        match lvl {
            4 => self.fatal += 1,
            3 => self.error += 1,
            2 => self.warn += 1,
            1 => self.info += 1,
            _ => self.debug += 1,
        }
    }

    /// Everything that asked for attention.
    pub fn loud(&self) -> u64 {
        self.fatal + self.error + self.warn
    }
}

/// A mod the game complained about while it ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModTrouble {
    pub folder: String,
    pub warnings: u32,
    pub errors: u32,
    /// the first few lines, as they were logged
    pub samples: Vec<String>,
}

/// One save the game wrote, or tried to.
///
/// A save is the one thing in this program's world that cannot be rebuilt from
/// anything else, so a write that failed, or that the game never finished, is
/// reported louder than a crash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveWritten {
    /// the file's own name, e.g. `save3.hg`
    pub file: String,
    pub bytes: u64,
    pub writes: u32,
    pub ms: u64,
    /// the Windows error, when a write failed
    pub error: Option<u32>,
    /// true when the game ended with the file still open
    pub unfinished: bool,
}

impl SaveWritten {
    pub fn went_wrong(&self) -> bool {
        self.error.is_some() || self.unfinished
    }
}

/// What the game's memory and handle use looked like at one moment.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Sample {
    pub at_ms: i64,
    pub working_set: u64,
    pub private: u64,
    pub peak_working_set: u64,
    pub handles: u64,
    pub page_faults: u64,
    /// physical memory the whole machine had left
    pub system_free: u64,
    pub system_total: u64,
}

/// How the game's memory use moved over a session.
///
/// Two samples and a count, not the whole series: the question this answers is
/// "did it keep growing", and for that the ends are the answer. The series is in
/// the log for anyone who wants to plot it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub first: Sample,
    pub last: Sample,
    pub samples: u32,
}

impl Memory {
    /// How much the working set grew, which can be negative.
    pub fn growth(&self) -> i64 {
        self.last.working_set as i64 - self.first.working_set as i64
    }
}

/// A file the game loaded instead of another copy of the same asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ignored {
    pub folder: String,
    /// the file that was never opened
    pub file: String,
    /// the file the game used instead
    pub instead: String,
}

/// One recorded session, as the Sessions list reads it.
///
/// This is the sidecar written beside the log, so the screen can show a run
/// without parsing a megabyte of text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brief {
    /// path of the text log this describes
    pub log: String,
    pub started_ms: i64,
    /// local time, spelled out
    pub started: String,
    pub seconds: u64,
    pub pid: u32,
    pub hook_version: String,
    pub exit_code: Option<i32>,
    /// what the exit code means, in words
    pub exit_note: String,
    pub crashed: bool,
    /// true when we stopped recording while the game was still running
    pub stopped_early: bool,
    pub counts: Counts,
    /// every save the game wrote this session, in order
    #[serde(default)]
    pub saves: Vec<SaveWritten>,
    /// what the game was running on, for the next session to compare against
    #[serde(default)]
    pub machine: Option<machine::Machine>,
    /// what changed since the session before this one -- a suspect list, not a
    /// diagnosis. See [`machine::changes`].
    #[serde(default)]
    pub changed: Vec<String>,
    /// how the game's memory use moved, when the session lasted long enough to
    /// be sampled at all
    #[serde(default)]
    pub memory: Option<Memory>,
    /// mod files the game actually opened
    pub files_opened: u64,
    pub mods_in_trouble: Vec<ModTrouble>,
    /// enabled mods the game opened nothing from
    pub never_loaded: Vec<String>,
    /// duplicate files the game demonstrably ignored
    pub ignored: Vec<Ignored>,
    /// the in-process crash report, when there was one
    pub crash: Option<String>,
    /// message boxes the game put up
    pub dialogs: Vec<String>,
    pub dumps: Vec<String>,
    /// one line: what this session amounted to
    pub verdict: String,
}

impl Brief {
    /// True when this session is worth the reader's attention.
    pub fn notable(&self) -> bool {
        self.crashed
            || self.counts.fatal > 0
            || self.counts.error > 0
            || !self.dialogs.is_empty()
            || self.saves.iter().any(SaveWritten::went_wrong)
    }
}

/// `3 h 22 m`, `12 m`, `48 s` -- a length a person weighs a session against.
pub fn spell_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {minutes} m")
        }
    } else if seconds >= 60 {
        format!("{} m", seconds / 60)
    } else {
        format!("{seconds} s")
    }
}

// ---------------------------------------------------------------------------
// the live line the screen shows
// ---------------------------------------------------------------------------

/// One event, ready to draw.
#[derive(Debug, Clone, Serialize)]
pub struct Live {
    pub ts: i64,
    /// local clock reading
    pub at: String,
    pub lvl: u8,
    pub cat: String,
    pub msg: String,
    /// the mod folder this is about, when it is about one
    pub owner: Option<String>,
}

// ---------------------------------------------------------------------------
// the recorder
// ---------------------------------------------------------------------------

/// Where the game is, for the log header.
#[derive(Debug, Clone)]
pub struct About {
    pub game_root: PathBuf,
    pub bin_dir: PathBuf,
    pub mods_dir: PathBuf,
    pub app_version: String,
    /// what this machine looks like now, read once as the session starts
    pub machine: Option<machine::Machine>,
    /// what changed since the last recorded session
    pub changed: Vec<String>,
}

#[derive(Default)]
struct Seen {
    warnings: u32,
    errors: u32,
    samples: Vec<String>,
    /// files of this mod the game opened, as reported
    opened: BTreeSet<String>,
}

/// Records one session: writes the log as it goes, and the brief at the end.
pub struct Recorder {
    log: std::fs::File,
    path: PathBuf,
    about: About,
    mods: Vec<ModFiles>,
    /// UPPER-CASED folder name -> the spelling it has on disk.
    ///
    /// The game asks for mod paths upper-cased -- `MODS\TERRALYSIS_PATH_OF_ORION\`
    /// for a folder called `TERRALYSIS_Path_Of_Orion` -- so without this every
    /// file the game opened belonged to a mod that was not in the library, and
    /// each of those mods was reported as having loaded nothing at all. It is
    /// also the name the rest of the app keys a mod by, so getting it right here
    /// is what lets the screen show a real title next to a finding.
    spelling: HashMap<String, String>,
    started_ms: i64,
    hello: Option<Hello>,
    counts: Counts,
    echo: Echo,
    seen: HashMap<String, Seen>,
    exceptions: HashMap<String, u64>,
    crashes: Vec<String>,
    exits: Vec<String>,
    dialogs: Vec<String>,
    heartbeat: Option<String>,
    saves: Vec<SaveWritten>,
    memory_first: Option<Sample>,
    memory_last: Option<Sample>,
    memory_samples: u32,
    files_opened: u64,
}

impl Recorder {
    /// Open a log for a session that has just connected.
    ///
    /// The header is written now rather than at the end, because the commonest
    /// reason to go looking for this file is that the game is currently
    /// misbehaving and the session has not finished.
    pub fn start(
        dir: &Path,
        about: About,
        mods: Vec<ModFiles>,
        hello: Option<Hello>,
    ) -> Result<Recorder, String> {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not make {}: {err}", dir.display()))?;
        let started_ms = clock::now_ms();
        let path = dir.join(format!("NMS_{}.log", clock::local(started_ms).stamp()));
        let log = std::fs::File::create(&path)
            .map_err(|err| format!("could not open {}: {err}", path.display()))?;

        let spelling = mods
            .iter()
            .map(|entry| (entry.folder.to_uppercase(), entry.folder.clone()))
            .collect();
        let mut recorder = Recorder {
            log,
            path,
            about,
            mods,
            spelling,
            started_ms,
            hello,
            counts: Counts::default(),
            echo: Echo::default(),
            seen: HashMap::new(),
            exceptions: HashMap::new(),
            crashes: Vec::new(),
            exits: Vec::new(),
            dialogs: Vec::new(),
            heartbeat: None,
            saves: Vec::new(),
            memory_first: None,
            memory_last: None,
            memory_samples: 0,
            files_opened: 0,
        };
        recorder.write_header();
        Ok(recorder)
    }

    pub fn log_path(&self) -> &Path {
        &self.path
    }

    pub fn started_ms(&self) -> i64 {
        self.started_ms
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    fn say(&mut self, text: &str) {
        let _ = self.log.write_all(text.as_bytes());
        let _ = self.log.write_all(b"\r\n");
    }

    fn rule(&mut self) {
        self.say(&"=".repeat(74));
    }

    fn write_header(&mut self) {
        let enabled = self.mods.iter().filter(|m| m.enabled).count();
        let at = clock::local(self.started_ms).written();
        let (pid, hook_version, exe, dropped) = match &self.hello {
            Some(hello) => (
                hello.pid,
                hello.hook_version.clone(),
                hello.exe.clone(),
                hello.backlog_dropped,
            ),
            None => (0, "?".to_string(), String::new(), 0),
        };
        let exe = if exe.is_empty() {
            hook::target(&self.about.bin_dir)
                .parent()
                .map(|dir| dir.join(gameproc::GAME_EXE).display().to_string())
                .unwrap_or_default()
        } else {
            exe
        };

        self.rule();
        self.say(&format!(" No Man's Sky session log  -  {at}"));
        self.rule();
        self.say(&format!(" game        : {exe}"));
        self.say(&format!(" game folder : {}", self.about.game_root.display()));
        self.say(&format!(
            " mods folder : {}  ({} mods, {enabled} enabled)",
            self.about.mods_dir.display(),
            self.mods.len()
        ));
        self.say(&format!(" pid         : {pid}"));
        self.say(&format!(" hook        : {hook_version}"));
        self.say(&format!(" recorded by : nmscheck {}", self.about.app_version));
        if dropped > 0 {
            self.say(&format!(
                " !! {dropped} events happened before we were listening and could not be kept"
            ));
        }
        if !self.about.changed.is_empty() {
            let changed = self.about.changed.clone();
            self.say("");
            self.say(" SINCE YOUR LAST RECORDED SESSION");
            for line in changed {
                self.say(&format!("   {line}"));
            }
            self.say(" Any of these can be the reason something behaves differently today.");
        }
        let off: Vec<&str> = self
            .mods
            .iter()
            .filter(|m| !m.enabled)
            .map(|m| m.folder.as_str())
            .collect();
        if !off.is_empty() {
            self.say(&format!(
                " switched off: {} ({})",
                off.len(),
                off.join(", ")
            ));
        }
        self.rule();
        self.say(" LIVE EVENTS");
        self.rule();
    }

    /// Take one line off the pipe. Returns the event, unless it was an echo.
    pub fn take(&mut self, line: &str) -> Option<Live> {
        match parse(line) {
            Line::Hello(hello) => {
                // A reconnect: the hook resends its greeting whenever the pipe
                // comes back. Noted, because a reconnect mid-session means the
                // pipe broke and something between the two may be missing.
                self.say(&format!(
                    "(the hook reconnected: process {}, version {})",
                    hello.pid, hello.hook_version
                ));
                if self.hello.is_none() {
                    self.hello = Some(hello);
                }
                None
            }
            Line::Junk(text) => {
                if !text.is_empty() {
                    self.say(&format!("(could not read this line) {text}"));
                }
                None
            }
            Line::Event(event) => self.record(event),
        }
    }

    fn record(&mut self, event: Event) -> Option<Live> {
        if self.echo.is_echo(&event.cat, &event.msg) {
            return None;
        }
        self.counts.add(event.lvl);
        let owner = self.track(&event);

        let at = clock::local(event.ts).clock();
        let level = LEVELS[usize::from(event.lvl).min(4)];
        let indented = event.msg.replace('\n', &format!("\n{}", " ".repeat(PREFIX)));
        let note = owner
            .as_ref()
            .map(|folder| format!("   <-- mod: {folder}"))
            .unwrap_or_default();
        self.say(&format!(
            "{at} {level} [{:<9}] {indented}{note}",
            event.cat
        ));

        Some(Live {
            ts: event.ts,
            at,
            lvl: event.lvl,
            cat: event.cat,
            msg: event.msg,
            owner,
        })
    }

    /// Update the tallies, and say which mod this event is about.
    fn track(&mut self, event: &Event) -> Option<String> {
        match event.cat.as_str() {
            "crash" if event.lvl == 4 => self.crashes.push(event.msg.clone()),
            "exit" => self.exits.push(event.msg.clone()),
            "dialog" => self.dialogs.push(event.msg.clone()),
            "hook" if event.msg.starts_with("heartbeat") => {
                self.heartbeat = Some(event.msg.clone())
            }
            "save" => {
                if let Some(save) = parse_save(&event.msg, event.ts) {
                    self.saves.push(save);
                }
            }
            "memory" => {
                if let Some(sample) = parse_sample(&event.msg, event.ts) {
                    if self.memory_first.is_none() {
                        self.memory_first = Some(sample);
                    }
                    self.memory_last = Some(sample);
                    self.memory_samples += 1;
                }
            }
            "exception" => {
                let module = exception_module(&event.msg);
                *self.exceptions.entry(module).or_insert(0) += 1;
            }
            _ => {}
        }

        // The recent-files list in a crash report is context, not blame.
        if matches!(event.cat.as_str(), "crash" | "module" | "hook") {
            return None;
        }
        let first = event.msg.lines().next().unwrap_or("");
        let (reported, rel) = mod_path(first)?;
        // Under the name the folder has on disk, not the upper-cased one the
        // game asked for -- a mod installed since the session began is not in
        // the list, and then what the game said is the best name there is.
        let folder = self
            .spelling
            .get(&reported.to_uppercase())
            .cloned()
            .unwrap_or(reported);
        let seen = self.seen.entry(folder.clone()).or_default();

        if event.cat == "modfile" && event.msg.starts_with("loaded ") {
            seen.opened.insert(rel);
            self.files_opened += 1;
            // A file loading is not a complaint about the mod, so the line
            // carries no blame note.
            return None;
        }
        if event.lvl >= 3 {
            seen.errors += 1;
        } else if event.lvl == 2 {
            seen.warnings += 1;
        }
        if event.lvl >= 2 && seen.samples.len() < SAMPLES {
            seen.samples.push(first.to_string());
        }
        Some(folder)
    }

    /// Write the summary, save the brief beside the log, and hand it back.
    pub fn finish(mut self, exit_code: Option<i32>, stopped_early: bool) -> Brief {
        let seconds = ((clock::now_ms() - self.started_ms).max(0) / 1000) as u64;
        let crashed = exit_code.map(gameproc::is_crash_code).unwrap_or(false);

        let after = crashinfo::collect(
            gameproc::GAME_EXE,
            self.started_ms,
            &crashinfo::dump_dirs(&self.about.game_root, &self.about.bin_dir),
        );

        let (never_loaded, ignored, waiting, useless) = self.usage();
        let mut in_trouble: Vec<ModTrouble> = self
            .seen
            .iter()
            .filter(|(_, seen)| seen.warnings > 0 || seen.errors > 0)
            .map(|(folder, seen)| ModTrouble {
                folder: folder.clone(),
                warnings: seen.warnings,
                errors: seen.errors,
                samples: seen.samples.clone(),
            })
            .collect();
        in_trouble.sort_by(|a, b| {
            (b.errors, b.warnings, a.folder.to_lowercase())
                .cmp(&(a.errors, a.warnings, b.folder.to_lowercase()))
        });

        let brief = Brief {
            log: self.path.display().to_string(),
            started_ms: self.started_ms,
            started: clock::local(self.started_ms).written(),
            seconds,
            pid: self.hello.as_ref().map(|h| h.pid).unwrap_or(0),
            hook_version: self
                .hello
                .as_ref()
                .map(|h| h.hook_version.clone())
                .unwrap_or_else(|| "?".to_string()),
            exit_code,
            exit_note: match exit_code {
                Some(code) => gameproc::describe_exit(code),
                None if stopped_early => "still running when recording stopped".to_string(),
                None => "unknown -- the game outlived the wait for it".to_string(),
            },
            crashed,
            stopped_early,
            counts: self.counts.clone(),
            saves: self.saves.clone(),
            machine: self.about.machine.clone(),
            changed: self.about.changed.clone(),
            memory: match (self.memory_first, self.memory_last) {
                (Some(first), Some(last)) => Some(Memory {
                    first,
                    last,
                    samples: self.memory_samples,
                }),
                _ => None,
            },
            files_opened: self.files_opened,
            mods_in_trouble: in_trouble,
            never_loaded,
            ignored,
            crash: self.crashes.first().cloned(),
            dialogs: self.dialogs.clone(),
            dumps: after.dumps.clone(),
            verdict: String::new(),
        };
        let brief = Brief {
            verdict: verdict(&brief),
            ..brief
        };

        self.write_summary(&brief, &after, &waiting, &useless);
        let _ = self.log.flush();

        // The sidecar is what the Sessions list reads. A failure to write it
        // costs the list one row and nothing else, so it is not raised: the log
        // itself, which is the record that matters, is already on disk.
        if let Ok(text) = serde_json::to_string_pretty(&brief) {
            let _ = std::fs::write(self.path.with_extension("json"), text);
        }
        brief
    }

    /// Which mod files went unopened, and which of those is a real finding.
    #[allow(clippy::type_complexity)]
    fn usage(&self) -> (Vec<String>, Vec<Ignored>, Vec<(String, Vec<String>)>, Vec<String>) {
        let mut never_loaded = Vec::new();
        let mut ignored = Vec::new();
        let mut waiting: Vec<(String, Vec<String>)> = Vec::new();
        let mut useless = Vec::new();

        for entry in self.mods.iter().filter(|m| m.enabled) {
            if entry.files.is_empty() {
                useless.push(entry.folder.clone());
                continue;
            }
            let empty = BTreeSet::new();
            let opened = self
                .seen
                .get(&entry.folder)
                .map(|seen| &seen.opened)
                .unwrap_or(&empty);

            // The hook reports the paths the game asked for, which are
            // upper-cased; the folder on disk is whatever the author typed.
            let opened_upper: HashSet<String> =
                opened.iter().map(|path| path.to_uppercase()).collect();
            let missing: Vec<&String> = entry
                .files
                .iter()
                .filter(|file| !opened_upper.contains(&file.to_uppercase()))
                .collect();

            if missing.len() == entry.files.len() {
                never_loaded.push(entry.folder.clone());
                continue;
            }
            let mut unexplained = Vec::new();
            for file in missing {
                let upper = file.to_uppercase();
                let stem = strip_data_ext(&upper).to_string();
                match opened_upper
                    .iter()
                    .find(|open| strip_data_ext(open) == stem && **open != upper)
                {
                    // Definite: the game loaded the same asset from a different
                    // file in this same mod, so this copy did nothing.
                    Some(used) => ignored.push(Ignored {
                        folder: entry.folder.clone(),
                        file: file.clone(),
                        instead: used.rsplit('\\').next().unwrap_or(used).to_string(),
                    }),
                    None => unexplained.push(file.clone()),
                }
            }
            if !unexplained.is_empty() {
                waiting.push((entry.folder.clone(), unexplained));
            }
        }
        never_loaded.sort();
        (never_loaded, ignored, waiting, useless)
    }

    fn write_summary(
        &mut self,
        brief: &Brief,
        after: &Aftermath,
        waiting: &[(String, Vec<String>)],
        useless: &[String],
    ) {
        self.say("");
        self.rule();
        self.say(" SESSION SUMMARY");
        self.rule();
        self.say(&format!(
            " duration : {} ({} seconds)",
            spell_duration(brief.seconds),
            brief.seconds
        ));
        match brief.exit_code {
            Some(code) => self.say(&format!(
                " exit     : code {code} (0x{:08X}) -- {}",
                code as u32, brief.exit_note
            )),
            None => self.say(&format!(" exit     : {}", brief.exit_note)),
        }
        let counts = brief.counts.clone();
        self.say(&format!(
            " events   : {} fatal, {} errors, {} warnings, {} info, {} debug",
            counts.fatal, counts.error, counts.warn, counts.info, counts.debug
        ));
        let exits = self.exits.clone();
        for said in exits {
            self.say(&format!(" hook saw : {said}"));
        }
        if let Some(beat) = self.heartbeat.clone() {
            self.say(&format!(" last hook status: {beat}"));
        }

        if !self.crashes.is_empty() {
            let reports = self.crashes.clone();
            self.say("");
            self.say(" CRASH REPORT, captured inside the game");
            for report in reports {
                self.say(&format!("   {}", report.replace('\n', "\n   ")));
            }
        } else if brief.crashed {
            // The pipe carries the report second and the hook's own file first,
            // so a crash too abrupt to send anything often still wrote it down.
            let path = hook::out_dir(&self.about.bin_dir).join(hook::RAW_LOG);
            self.say("");
            self.say(&format!(
                " THE GAME CRASHED, and no report reached us. The end of {}:",
                path.display()
            ));
            for line in crashinfo::tail(&path, 60) {
                self.say(&format!("   {line}"));
            }
        }

        if !self.dialogs.is_empty() {
            let dialogs = self.dialogs.clone();
            self.say("");
            self.say(" MESSAGE BOXES THE GAME PUT UP");
            for said in dialogs {
                self.say(&format!("   {}", said.replace('\n', "\n   ")));
            }
        }

        // Before the mods and before the file usage, because a save that did not
        // get written is the only thing here a person has to act on tonight.
        if !brief.saves.is_empty() {
            let bad: Vec<&SaveWritten> = brief.saves.iter().filter(|s| s.went_wrong()).collect();
            self.say("");
            if bad.is_empty() {
                let total: u64 = brief.saves.iter().map(|s| s.bytes).sum();
                self.say(&format!(
                    " SAVES  {} written, {} in all, all of them completed",
                    brief.saves.len(),
                    big_bytes(total)
                ));
            } else {
                self.say(" SAVES  SOMETHING WENT WRONG WRITING A SAVE");
                for save in bad {
                    match save.error {
                        Some(code) => self.say(&format!(
                            "   {}: failed with Windows error {code} after {}",
                            save.file,
                            big_bytes(save.bytes)
                        )),
                        None => self.say(&format!(
                            "   {}: the game ended with it still open, {} written",
                            save.file,
                            big_bytes(save.bytes)
                        )),
                    }
                }
                self.say("   A save that did not finish writing may be short. Check the file's size");
                self.say("   against the others, and keep a backup of the one from before this session.");
            }
            for save in brief.saves.iter().filter(|s| !s.went_wrong()) {
                self.say(&format!(
                    "   {}: {} in {} writes over {} ms",
                    save.file,
                    big_bytes(save.bytes),
                    save.writes,
                    save.ms
                ));
            }
        }

        if let Some(memory) = &brief.memory {
            self.say("");
            self.say(&format!(
                " MEMORY  working set {} at the first sample, {} at the last ({}{}) over {} samples",
                big_bytes(memory.first.working_set),
                big_bytes(memory.last.working_set),
                if memory.growth() >= 0 { "+" } else { "-" },
                big_bytes(memory.growth().unsigned_abs()),
                memory.samples
            ));
            self.say(&format!(
                "         peak {}, handles {} -> {}, machine had {} of {} free at the end",
                big_bytes(memory.last.peak_working_set),
                memory.first.handles,
                memory.last.handles,
                big_bytes(memory.last.system_free),
                big_bytes(memory.last.system_total)
            ));
        }

        if !brief.mods_in_trouble.is_empty() {
            self.say("");
            self.say(" MODS THE GAME COMPLAINED ABOUT");
            for mod_ in brief.mods_in_trouble.clone() {
                self.say(&format!(
                    "   {}: {} errors, {} warnings",
                    mod_.folder, mod_.errors, mod_.warnings
                ));
                for sample in mod_.samples {
                    self.say(&format!("       {sample}"));
                }
            }
        }

        self.say("");
        if brief.files_opened == 0 {
            self.say(" MOD FILE USAGE  the game opened no mod files at all this session, so this");
            self.say("                 check was skipped. Normal if it never reached the main menu.");
        } else {
            self.say(&format!(
                " MOD FILE USAGE  the game opened {} mod files",
                brief.files_opened
            ));
            for folder in useless {
                self.say(&format!(
                    "   {folder}: nothing the game can load, so it cannot do anything"
                ));
            }
            if !brief.ignored.is_empty() {
                self.say("   Files the game IGNORED, because it loaded the same asset from another:");
                for entry in brief.ignored.clone() {
                    self.say(&format!(
                        "     {}: {} -- the game used {} instead",
                        entry.folder, entry.file, entry.instead
                    ));
                }
            }
            if !brief.never_loaded.is_empty() {
                self.say("   Enabled mods the game opened nothing from this session:");
                for folder in brief.never_loaded.clone() {
                    self.say(&format!("     {folder}"));
                }
            }
            if !waiting.is_empty() {
                self.say("   Files the game did not open. Either it never asked for them -- situational");
                self.say("   content, or a path this version of the game no longer uses -- or it skipped them:");
                for (folder, files) in waiting {
                    self.say(&format!("     {folder}:"));
                    for file in files.iter().take(15) {
                        self.say(&format!("         {file}"));
                    }
                    if files.len() > 15 {
                        self.say(&format!("         ... and {} more", files.len() - 15));
                    }
                }
            }
        }

        if !self.exceptions.is_empty() {
            let mut by_module: Vec<(String, u64)> =
                self.exceptions.iter().map(|(k, v)| (k.clone(), *v)).collect();
            by_module.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            self.say("");
            self.say(" EXCEPTIONS BY MODULE (first-chance; the game handles most of these itself)");
            for (module, count) in by_module {
                self.say(&format!("   {count:>5}  {module}"));
            }
        }

        if !after.events.is_empty() {
            self.say("");
            self.say(" WINDOWS APPLICATION LOG");
            for event in after.events.clone() {
                self.say(&format!(
                    "   [{}] {} (event {}):\n      {}",
                    event.at, event.provider, event.id, event.summary
                ));
            }
        }
        if let Some(note) = after.note.clone() {
            self.say(&format!("   ({note})"));
        }
        if !after.dumps.is_empty() {
            self.say("");
            self.say(" CRASH DUMPS WRITTEN DURING THIS SESSION");
            for path in after.dumps.clone() {
                self.say(&format!("   {path}"));
            }
        }
        self.rule();
    }
}

/// Read one of the hook's `save` lines.
///
/// The hook writes these for a person first -- "wrote save3.hg: 786432 bytes in
/// 12 writes over 380 ms (0.8 MB)" -- so they are read by looking for the file
/// name and then for each number by the word that follows it, rather than by
/// position. A line whose wording changes still yields whatever it still says.
fn parse_save(msg: &str, ts: i64) -> Option<SaveWritten> {
    let _ = ts;
    let file = msg
        .split_whitespace()
        .find(|word| word.trim_end_matches([':', ',']).to_lowercase().ends_with(".hg"))
        .map(|word| word.trim_end_matches([':', ',']).to_string())?;

    // `bytes`, `writes` and `ms` each follow their number; `error` precedes it.
    let words: Vec<&str> = msg.split_whitespace().collect();
    let before = |unit: &str| -> Option<u64> {
        words
            .iter()
            .position(|word| *word == unit)
            .and_then(|at| at.checked_sub(1))
            .and_then(|at| words[at].replace(',', "").parse().ok())
    };
    let after = |label: &str| -> Option<u64> {
        words
            .iter()
            .position(|word| *word == label)
            .and_then(|at| words.get(at + 1))
            .and_then(|word| word.trim_end_matches([':', ',']).parse().ok())
    };

    Some(SaveWritten {
        file,
        bytes: before("bytes").unwrap_or(0),
        writes: before("writes").unwrap_or(0) as u32,
        ms: before("ms").unwrap_or(0),
        error: after("error").map(|code| code as u32),
        unfinished: msg.contains("still open"),
    })
}

/// Read one of the hook's `memory` lines: `key=value` pairs, all in bytes.
fn parse_sample(msg: &str, ts: i64) -> Option<Sample> {
    let mut sample = Sample {
        at_ms: ts,
        ..Sample::default()
    };
    let mut found = false;
    for pair in msg.split_whitespace() {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let Ok(number) = value.parse::<u64>() else {
            continue;
        };
        found = true;
        match key {
            "workingSet" => sample.working_set = number,
            "peakWorkingSet" => sample.peak_working_set = number,
            "private" => sample.private = number,
            "handles" => sample.handles = number,
            "pageFaults" => sample.page_faults = number,
            "systemFree" => sample.system_free = number,
            "systemTotal" => sample.system_total = number,
            // saveBytes, saveWrites and saveFailures ride along on the same line;
            // the individual save events say it better, so they are not kept here.
            _ => {}
        }
    }
    found.then_some(sample)
}

/// `1.4 GB`, `812 MB` -- for a summary a person reads, never for a comparison.
fn big_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut at = 0;
    while size >= 1024.0 && at < UNITS.len() - 1 {
        size /= 1024.0;
        at += 1;
    }
    if at == 0 {
        format!("{bytes} B")
    } else if size < 10.0 {
        format!("{size:.1} {}", UNITS[at])
    } else {
        format!("{size:.0} {}", UNITS[at])
    }
}

/// The module an exception came from: `NMS.exe` out of `... at NMS.exe+0x11E1`.
fn exception_module(msg: &str) -> String {
    let tail = msg
        .rfind(" at ")
        .map(|at| &msg[at + 4..])
        .or_else(|| msg.rfind(" near ").map(|at| &msg[at + 6..]))
        .unwrap_or("?");
    tail.split(['+', ' ']).next().unwrap_or("?").to_string()
}

/// Pull the mod folder and the file inside it out of a path in a message.
///
/// Case-insensitive, because the game asks for these paths upper-cased while the
/// folder on disk is spelled however the author spelled it.
pub fn mod_path(line: &str) -> Option<(String, String)> {
    const NEEDLE: &str = "\\GAMEDATA\\MODS\\";
    let upper = line.to_uppercase();
    let at = upper.find(NEEDLE)? + NEEDLE.len();
    let rest = &line[at..];
    let (folder, rel) = rest.split_once('\\')?;
    if folder.is_empty() || rel.is_empty() {
        return None;
    }
    Some((folder.to_string(), rel.to_string()))
}

/// `X.MBIN` and `X.EXML` are the same asset; compare them by what is left.
fn strip_data_ext(path: &str) -> &str {
    for ext in DATA_EXTS {
        if path.len() > ext.len() && path.to_uppercase().ends_with(ext) {
            return &path[..path.len() - ext.len()];
        }
    }
    path
}

/// One sentence for what a session amounted to.
///
/// Written to be read *instead of* the log in the common case. The order is the
/// order a reader cares about: did it crash, did anything shout, how long was it.
fn verdict(brief: &Brief) -> String {
    let ran = spell_duration(brief.seconds);
    // Ahead of the crash, and deliberately: a crash costs the session, a save
    // that did not write costs the save, and only one of those is recoverable
    // by playing again.
    if let Some(bad) = brief.saves.iter().find(|save| save.went_wrong()) {
        return match bad.error {
            Some(code) => format!(
                "A SAVE FAILED TO WRITE after {ran} -- {} stopped with Windows error {code}",
                bad.file
            ),
            None => format!(
                "A SAVE MAY BE INCOMPLETE after {ran} -- the game ended while writing {}",
                bad.file
            ),
        };
    }
    if brief.crashed {
        let where_ = brief
            .crash
            .as_deref()
            .and_then(crash_headline)
            .unwrap_or_else(|| brief.exit_note.clone());
        return format!("Crashed after {ran} -- {where_}");
    }
    if brief.stopped_early {
        return format!("Still running after {ran} when recording stopped");
    }
    let mut said = match brief.exit_code {
        Some(0) | None => format!("Ran {ran}"),
        Some(code) => format!("Ran {ran}, then exited with code {code}"),
    };
    let counts = &brief.counts;
    if counts.error + counts.fatal > 0 {
        said += &format!(
            " -- {} error{} from {} mod{}",
            counts.error + counts.fatal,
            plural(counts.error + counts.fatal),
            brief.mods_in_trouble.len(),
            plural(brief.mods_in_trouble.len() as u64)
        );
    } else if counts.warn > 0 {
        said += &format!(" -- {} warning{}", counts.warn, plural(counts.warn));
    } else if brief.files_opened > 0 {
        said += " -- nothing reported";
    } else {
        said += " -- the game opened no mod files";
    }
    said
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// `access violation at NMS.exe+0x11E1`, out of the hook's crash report.
fn crash_headline(report: &str) -> Option<String> {
    let mut exception = None;
    let mut location = None;
    for line in report.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("exception:") {
            // `0xC0000005 ACCESS_VIOLATION (writing address 0x8)` -> the name.
            exception = rest
                .split_whitespace()
                .nth(1)
                .map(|name| name.replace('_', " ").to_lowercase());
        } else if let Some(rest) = line.strip_prefix("location:") {
            location = Some(rest.trim().to_string());
        }
    }
    match (exception, location) {
        (Some(what), Some(where_)) => Some(format!("{what} at {where_}")),
        (Some(what), None) => Some(what),
        (None, Some(where_)) => Some(format!("crashed at {where_}")),
        (None, None) => None,
    }
}

// ---------------------------------------------------------------------------
// the sessions on disk
// ---------------------------------------------------------------------------

/// Every recorded session, newest first.
///
/// Read from the sidecars, so a folder of thirty logs costs thirty small reads
/// rather than thirty megabytes. A log whose sidecar is missing -- a session
/// interrupted by the app closing, or by a crash of ours -- is still listed,
/// with what the file name says and nothing invented.
pub fn list(dir: &Path) -> Vec<Brief> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Brief> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        match super::read_json::<Brief>(&path.with_extension("json")) {
            Some(brief) => found.push(brief),
            None => found.push(unfinished(&path)),
        }
    }
    found.sort_by_key(|brief| std::cmp::Reverse(brief.started_ms));
    found
}

/// A session whose summary was never written.
fn unfinished(path: &Path) -> Brief {
    let started_ms = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Brief {
        log: path.display().to_string(),
        started_ms,
        started: clock::local(started_ms).written(),
        seconds: 0,
        pid: 0,
        hook_version: "?".to_string(),
        exit_code: None,
        exit_note: "this session was never finished off".to_string(),
        crashed: false,
        stopped_early: true,
        counts: Counts::default(),
        saves: Vec::new(),
        machine: None,
        changed: Vec::new(),
        memory: None,
        files_opened: 0,
        mods_in_trouble: Vec::new(),
        never_loaded: Vec::new(),
        ignored: Vec::new(),
        crash: None,
        dialogs: Vec::new(),
        dumps: Vec::new(),
        verdict: "Recording was interrupted, so this session has no summary".to_string(),
    }
}

/// Read a session log for the viewer, keeping the end when it is huge.
///
/// A session with `LogAllFileOpens` on can run to megabytes, and the end is the
/// part that matters -- the summary is there, and so is whatever happened last.
pub fn read_log(path: &Path, most: usize) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|err| format!("could not read {}: {err}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if text.len() <= most {
        return Ok(text.to_string());
    }
    // Cut on a line boundary, so the viewer never opens mid-word.
    let tail = &text[text.len() - most..];
    let from = tail.find('\n').map(|at| at + 1).unwrap_or(0);
    Ok(format!(
        "(this log is {} long; showing the last {} -- open the file for all of it)\n\n{}",
        big(text.len()),
        big(most),
        &tail[from..]
    ))
}

fn big(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// Delete one session: the log and its sidecar together.
pub fn forget(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|err| format!("could not delete {}: {err}", path.display()))?;
    let _ = std::fs::remove_file(path.with_extension("json"));
    Ok(())
}

/// Keep the newest `keep` sessions and delete the rest.
///
/// Called after a session finishes rather than on a timer: that is the one
/// moment a new file exists, and it means the folder never grows past the limit
/// even if the app is never opened again.
pub fn prune(dir: &Path, keep: usize) -> usize {
    let sessions = list(dir);
    let mut gone = 0;
    for brief in sessions.into_iter().skip(keep) {
        if forget(Path::new(&brief.log)).is_ok() {
            gone += 1;
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_sessionlog_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn about(dir: &Path) -> About {
        About {
            game_root: dir.join("game"),
            bin_dir: dir.join("game").join("Binaries"),
            mods_dir: dir.join("game").join("GAMEDATA").join("MODS"),
            app_version: "test".to_string(),
            machine: None,
            changed: Vec::new(),
        }
    }

    fn event(lvl: u8, cat: &str, msg: &str) -> String {
        serde_json::json!({ "ts": 1_790_370_766_123i64, "lvl": lvl, "tid": 7, "cat": cat, "msg": msg })
            .to_string()
    }

    #[test]
    fn a_hello_is_read_as_the_greeting_and_not_as_an_event() {
        let line = r#"{"type":"hello","pid":4612,"hookVersion":"0.1.0","exe":"D:\\NMS.exe","backlogDropped":3}"#;
        match parse(line) {
            Line::Hello(hello) => {
                assert_eq!(hello.pid, 4612);
                assert_eq!(hello.hook_version, "0.1.0");
                assert_eq!(hello.backlog_dropped, 3);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_event_keeps_its_level_and_category() {
        match parse(&event(2, "modfile", "open failed (error 2): X")) {
            Line::Event(e) => {
                assert_eq!(e.lvl, 2);
                assert_eq!(e.cat, "modfile");
                assert_eq!(e.ts, 1_790_370_766_123);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_line_we_cannot_read_is_kept_as_junk_rather_than_dropped() {
        assert!(matches!(parse("not json at all"), Line::Junk(_)));
        // A control line from a newer hook is not an event either.
        assert!(matches!(parse(r#"{"type":"goodbye"}"#), Line::Junk(_)));
    }

    #[test]
    fn an_event_from_a_newer_hook_with_extra_fields_still_reads() {
        let line = r#"{"ts":1,"lvl":3,"tid":2,"cat":"gamelog","msg":"boom","somethingNew":true}"#;
        match parse(line) {
            Line::Event(e) => assert_eq!(e.msg, "boom"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_second_copy_of_a_game_log_line_is_dropped() {
        let mut echo = Echo::default();
        assert!(!echo.is_echo("gamelog", "shader cache missing"));
        assert!(echo.is_echo("debugout", "[NMS.exe] shader cache missing"));
        // And the other way round: whichever arrives first is the one kept.
        assert!(!echo.is_echo("debugout", "[NMS.exe] second line"));
        assert!(echo.is_echo("gamelog", "second line"));
    }

    #[test]
    fn a_debug_line_from_another_module_is_not_an_echo() {
        let mut echo = Echo::default();
        assert!(!echo.is_echo("gamelog", "loading"));
        assert!(!echo.is_echo("debugout", "[nvngx.dll] loading"));
    }

    #[test]
    fn events_in_other_categories_are_never_echoes() {
        let mut echo = Echo::default();
        assert!(!echo.is_echo("modfile", "loaded X"));
        assert!(!echo.is_echo("modfile", "loaded X"));
    }

    #[test]
    fn a_mod_path_yields_its_folder_and_the_file_inside_it() {
        let line = r"open failed (error 2): D:\Game\GAMEDATA\MODS\Better Colors\TEXTURES\A.DDS";
        let (folder, rel) = mod_path(line).unwrap();
        assert_eq!(folder, "Better Colors");
        assert_eq!(rel, r"TEXTURES\A.DDS");
    }

    #[test]
    fn a_mod_path_is_found_whatever_case_the_game_asked_in() {
        let line = r"loaded d:\game\gamedata\mods\ALPHA\METADATA\X.MBIN";
        let (folder, rel) = mod_path(line).unwrap();
        assert_eq!(folder, "ALPHA");
        assert_eq!(rel, r"METADATA\X.MBIN");
    }

    #[test]
    fn a_line_with_no_mod_in_it_yields_nothing() {
        assert!(mod_path(r"D:\Game\GAMEDATA\PCBANKS\A.pak").is_none());
        // The MODS folder itself is not a mod.
        assert!(mod_path(r"D:\Game\GAMEDATA\MODS\Alpha").is_none());
    }

    #[test]
    fn an_exception_is_tallied_against_its_module() {
        assert_eq!(
            exception_module("first-chance 0xC0000005 ACCESS_VIOLATION at NMS.exe+0x11E1 (may be handled)"),
            "NMS.exe"
        );
        assert_eq!(
            exception_module("C++ exception thrown (first-chance) near nvwgf2umx.dll+0x3F0"),
            "nvwgf2umx.dll"
        );
        assert_eq!(exception_module("something we have not seen"), "?");
    }

    #[test]
    fn documentation_is_not_something_the_game_can_load() {
        assert!(!is_game_file("readme.txt"));
        assert!(!is_game_file("SCRIPT.lua"));
        assert!(!is_game_file("__folder_managed_by_vortex"));
        assert!(is_game_file("GCGAMEPLAYGLOBALS.GLOBAL.MBIN"));
        assert!(is_game_file("URANIUM.DDS"));
    }

    #[test]
    fn a_finished_save_is_read_off_the_hooks_own_wording() {
        let save = parse_save(
            "wrote save3.hg: 28715904 bytes in 12 writes over 380 ms (27.4 MB)",
            1,
        )
        .unwrap();
        assert_eq!(save.file, "save3.hg");
        assert_eq!(save.bytes, 28_715_904);
        assert_eq!(save.writes, 12);
        assert_eq!(save.ms, 380);
        assert!(save.error.is_none());
        assert!(!save.went_wrong());
    }

    #[test]
    fn a_failed_save_carries_the_windows_error() {
        let save =
            parse_save("FAILED writing mf_save3.hg: error 112 after 4096 bytes in 3 writes over 12 ms", 1)
                .unwrap();
        assert_eq!(save.file, "mf_save3.hg");
        assert_eq!(save.error, Some(112));
        assert_eq!(save.bytes, 4096);
        assert!(save.went_wrong());
    }

    #[test]
    fn a_save_the_game_never_closed_is_flagged_as_unfinished() {
        let save = parse_save(
            "save3.hg was still open when the game ended: 12288 bytes in 2 writes",
            1,
        )
        .unwrap();
        assert!(save.unfinished);
        assert!(save.went_wrong());
        assert_eq!(save.bytes, 12_288);
        // No duration in this wording, and inventing one would be worse than 0.
        assert_eq!(save.ms, 0);
    }

    #[test]
    fn a_line_that_names_no_save_file_is_not_a_save() {
        assert!(parse_save("something about saving in general", 1).is_none());
    }

    #[test]
    fn a_memory_sample_is_read_as_key_value_pairs() {
        let sample = parse_sample(
            "workingSet=11644928 peakWorkingSet=12800000 private=2314240 peakPaged=3276800 \
             pageFaults=53145 handles=170 systemFree=7490138112 systemTotal=34004492288 \
             saveBytes=786432 saveWrites=13 saveFailures=1",
            77,
        )
        .unwrap();
        assert_eq!(sample.at_ms, 77);
        assert_eq!(sample.working_set, 11_644_928);
        assert_eq!(sample.handles, 170);
        assert_eq!(sample.system_total, 34_004_492_288);
        // An unknown key is ignored rather than refused: a newer hook may send
        // fields this build has never heard of.
        assert!(parse_sample("workingSet=1 futureThing=2", 0).is_some());
        assert!(parse_sample("nothing parseable here", 0).is_none());
    }

    #[test]
    fn a_failed_save_leads_the_verdict_even_over_a_crash() {
        let dir = Dir::new("savefail");
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), Vec::new(), None).unwrap();
        rec.take(&event(
            3,
            "save",
            "FAILED writing save3.hg: error 112 after 4096 bytes in 3 writes over 12 ms",
        ));
        rec.take(&event(
            4,
            "crash",
            "UNHANDLED EXCEPTION - the game is crashing\n  exception: 0xC0000005 ACCESS_VIOLATION\n  location:  NMS.exe+0x1",
        ));
        let brief = rec.finish(Some(0xC000_0005u32 as i32), false);

        assert!(brief.crashed, "it did crash, and the summary still says so");
        assert!(
            brief.verdict.starts_with("A SAVE FAILED"),
            "a crash costs the session; a save that did not write costs the save: {}",
            brief.verdict
        );
        assert!(brief.notable());
        assert_eq!(brief.saves.len(), 1);
        let text = std::fs::read_to_string(&brief.log).unwrap();
        assert!(text.contains("SOMETHING WENT WRONG WRITING A SAVE"), "{text}");
    }

    #[test]
    fn memory_growth_is_measured_from_the_first_sample_to_the_last() {
        let dir = Dir::new("memory");
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), Vec::new(), None).unwrap();
        rec.take(&event(0, "memory", "workingSet=1000000 handles=100 systemFree=8000000000"));
        rec.take(&event(0, "memory", "workingSet=3000000 handles=140 systemFree=6000000000"));
        let brief = rec.finish(Some(0), false);

        let memory = brief.memory.expect("two samples make a trend");
        assert_eq!(memory.samples, 2);
        assert_eq!(memory.growth(), 2_000_000);
        assert_eq!(memory.last.handles, 140);
        let text = std::fs::read_to_string(&brief.log).unwrap();
        assert!(text.contains("MEMORY"), "{text}");
    }

    #[test]
    fn a_session_too_short_to_be_sampled_has_no_memory_trend() {
        let dir = Dir::new("nomemory");
        let rec = Recorder::start(&dir.0.join("sessions"), about(&dir.0), Vec::new(), None).unwrap();
        assert!(rec.finish(Some(0), false).memory.is_none());
    }

    #[test]
    fn sizes_are_spelled_for_a_reader_without_being_wrong() {
        assert_eq!(big_bytes(512), "512 B");
        assert_eq!(big_bytes(28_715_904), "27 MB");
        assert_eq!(big_bytes(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn a_crash_report_is_reduced_to_one_line() {
        let report = "UNHANDLED EXCEPTION - the game is crashing\n  \
                      exception: 0xC0000005 ACCESS_VIOLATION (writing address 0x8)\n  \
                      location:  NMS.exe+0x11E1\n  thread:    1234\n";
        assert_eq!(
            crash_headline(report).unwrap(),
            "access violation at NMS.exe+0x11E1"
        );
    }

    #[test]
    fn durations_are_spelled_the_way_a_person_would_say_them() {
        assert_eq!(spell_duration(48), "48 s");
        assert_eq!(spell_duration(750), "12 m");
        assert_eq!(spell_duration(12_131), "3 h 22 m");
        assert_eq!(spell_duration(7_200), "2 h");
    }

    #[test]
    fn a_session_records_events_and_writes_a_log() {
        let dir = Dir::new("records");
        let mut rec = Recorder::start(
            &dir.0.join("sessions"),
            about(&dir.0),
            Vec::new(),
            Some(Hello {
                pid: 99,
                hook_version: "0.1.0".into(),
                exe: r"D:\NMS.exe".into(),
                backlog_dropped: 0,
            }),
        )
        .unwrap();

        assert!(rec.take(&event(2, "gamelog", "something failed")).is_some());
        // ... and its echo is not recorded twice.
        assert!(rec
            .take(&event(2, "debugout", "[NMS.exe] something failed"))
            .is_none());
        let path = rec.log_path().to_path_buf();
        let brief = rec.finish(Some(0), false);

        assert_eq!(brief.counts.warn, 1, "the echo must not have been counted");
        assert_eq!(brief.exit_code, Some(0));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("LIVE EVENTS"));
        assert!(text.contains("something failed"));
        assert!(text.contains("SESSION SUMMARY"));
        assert!(text.contains("pid         : 99"));
        // And the brief is beside it, for the list.
        assert!(path.with_extension("json").is_file());
    }

    #[test]
    fn a_mod_that_shouted_is_named_with_an_example() {
        let dir = Dir::new("trouble");
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), Vec::new(), None).unwrap();
        rec.take(&event(
            3,
            "gamelog",
            r"failed to parse D:\Game\GAMEDATA\MODS\Alpha\METADATA\X.EXML",
        ));
        let brief = rec.finish(Some(0), false);

        assert_eq!(brief.mods_in_trouble.len(), 1);
        assert_eq!(brief.mods_in_trouble[0].folder, "Alpha");
        assert_eq!(brief.mods_in_trouble[0].errors, 1);
        assert_eq!(brief.mods_in_trouble[0].samples.len(), 1);
        assert!(brief.verdict.contains("1 error"), "{}", brief.verdict);
    }

    #[test]
    fn a_mod_named_only_in_a_crash_report_is_not_blamed() {
        let dir = Dir::new("blameless");
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), Vec::new(), None).unwrap();
        rec.take(&event(
            4,
            "crash",
            "UNHANDLED EXCEPTION\n  recent files:\n    D:\\Game\\GAMEDATA\\MODS\\Alpha\\A.MBIN ok",
        ));
        let brief = rec.finish(Some(0xC000_0005u32 as i32), false);

        assert!(
            brief.mods_in_trouble.is_empty(),
            "the crash listed the mod as context, not as a cause"
        );
        assert!(brief.crash.is_some());
        assert!(brief.crashed);
        assert!(brief.verdict.starts_with("Crashed"), "{}", brief.verdict);
    }

    #[test]
    fn a_duplicate_file_the_game_skipped_is_reported_as_definitely_dead() {
        let dir = Dir::new("ignored");
        let mods = vec![ModFiles {
            folder: "Alpha".into(),
            enabled: true,
            files: vec![
                r"METADATA\X.MBIN".into(),
                r"METADATA\X.MXML".into(),
            ],
        }];
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), mods, None).unwrap();
        rec.take(&event(
            1,
            "modfile",
            r"loaded D:\GAME\GAMEDATA\MODS\ALPHA\METADATA\X.MBIN",
        ));
        let brief = rec.finish(Some(0), false);

        assert_eq!(brief.files_opened, 1);
        assert_eq!(brief.ignored.len(), 1, "{:?}", brief.ignored);
        assert_eq!(brief.ignored[0].file, r"METADATA\X.MXML");
        assert_eq!(brief.ignored[0].instead, "X.MBIN");
        assert!(brief.never_loaded.is_empty());
    }

    #[test]
    fn an_enabled_mod_the_game_opened_nothing_from_is_listed_apart() {
        let dir = Dir::new("never");
        let mods = vec![
            ModFiles {
                folder: "Alpha".into(),
                enabled: true,
                files: vec![r"METADATA\X.MBIN".into()],
            },
            ModFiles {
                folder: "Beta".into(),
                enabled: false,
                files: vec![r"METADATA\Y.MBIN".into()],
            },
        ];
        let mut rec =
            Recorder::start(&dir.0.join("sessions"), about(&dir.0), mods, None).unwrap();
        rec.take(&event(
            1,
            "modfile",
            r"loaded D:\GAME\GAMEDATA\MODS\SOMETHING ELSE\METADATA\Z.MBIN",
        ));
        let brief = rec.finish(Some(0), false);

        assert_eq!(brief.never_loaded, vec!["Alpha".to_string()]);
        assert!(
            !brief.never_loaded.contains(&"Beta".to_string()),
            "a mod that is switched off was not expected to load"
        );
    }

    #[test]
    fn a_mod_the_game_named_in_upper_case_is_still_that_mod() {
        // The game asks for `MODS\TERRALYSIS_PATH_OF_ORION\...` for a folder
        // called `TERRALYSIS_Path_Of_Orion`. Keyed by what the game said, every
        // one of its files counted against a mod nobody has, and the real mod
        // was reported as having loaded nothing.
        let dir = Dir::new("case");
        let mods = vec![ModFiles {
            folder: "TERRALYSIS_Path_Of_Orion".into(),
            enabled: true,
            files: vec![r"METADATA\X.MBIN".into()],
        }];
        let mut rec = Recorder::start(&dir.0.join("sessions"), about(&dir.0), mods, None).unwrap();
        let live = rec
            .take(&event(
                2,
                "modfile",
                r"open failed (error 2): D:\GAME\GAMEDATA\MODS\TERRALYSIS_PATH_OF_ORION\METADATA\X.MBIN",
            ))
            .unwrap();
        assert_eq!(
            live.owner.as_deref(),
            Some("TERRALYSIS_Path_Of_Orion"),
            "the name the rest of the app knows the mod by"
        );
        let brief = rec.finish(Some(0), false);
        assert_eq!(brief.mods_in_trouble[0].folder, "TERRALYSIS_Path_Of_Orion");
    }

    #[test]
    fn the_inventory_reads_folders_and_skips_documentation() {
        let dir = Dir::new("inventory");
        dir.file(r"MODS\Alpha\METADATA\X.MBIN", "x");
        dir.file(r"MODS\Alpha\readme.txt", "hello");
        dir.file(r"MODS\Beta\SCRIPT.lua", "-- amumss");
        let found = inventory(&dir.0.join("MODS"), &dir.0.join("game"));

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].folder, "Alpha");
        assert_eq!(found[0].files, vec![r"METADATA\X.MBIN".to_string()]);
        assert!(
            found[1].files.is_empty(),
            "a mod of nothing but a Lua script cannot do anything"
        );
    }

    #[test]
    fn sessions_are_listed_newest_first_and_a_missing_summary_still_shows() {
        let dir = Dir::new("list");
        let sessions = dir.0.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        for (name, started) in [("NMS_a.log", 1_000i64), ("NMS_b.log", 2_000i64)] {
            let log = sessions.join(name);
            std::fs::write(&log, "body").unwrap();
            let brief = Brief {
                log: log.display().to_string(),
                started_ms: started,
                started: "then".into(),
                seconds: 1,
                pid: 0,
                hook_version: "0.1.0".into(),
                exit_code: Some(0),
                exit_note: "normal exit".into(),
                crashed: false,
                stopped_early: false,
                counts: Counts::default(),
                saves: Vec::new(),
                machine: None,
                changed: Vec::new(),
                memory: None,
                files_opened: 0,
                mods_in_trouble: Vec::new(),
                never_loaded: Vec::new(),
                ignored: Vec::new(),
                crash: None,
                dialogs: Vec::new(),
                dumps: Vec::new(),
                verdict: "fine".into(),
            };
            std::fs::write(
                log.with_extension("json"),
                serde_json::to_string(&brief).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(sessions.join("NMS_c.log"), "interrupted").unwrap();

        let found = list(&sessions);
        assert_eq!(found.len(), 3);
        // The interrupted one has no brief, so its time comes from the file --
        // written just now, which puts it first. What matters is that the two
        // with briefs are in the order their briefs say.
        let with_briefs: Vec<i64> = found
            .iter()
            .filter(|b| b.started_ms < 10_000)
            .map(|b| b.started_ms)
            .collect();
        assert_eq!(with_briefs, vec![2_000, 1_000]);
        let orphan = found.iter().find(|b| b.log.ends_with("NMS_c.log")).unwrap();
        assert!(orphan.verdict.contains("interrupted"));
    }

    #[test]
    fn pruning_keeps_the_newest_and_takes_the_sidecar_with_it() {
        let dir = Dir::new("prune");
        let sessions = dir.0.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for name in ["NMS_1.log", "NMS_2.log", "NMS_3.log"] {
            std::fs::write(sessions.join(name), "body").unwrap();
            std::fs::write(sessions.join(name.replace(".log", ".json")), "{}").unwrap();
        }
        // No readable sidecars, so order falls back to file times -- all three
        // are the same age here, which is fine: the count is what is asserted.
        assert_eq!(prune(&sessions, 1), 2);
        let left = list(&sessions);
        assert_eq!(left.len(), 1);
        assert_eq!(
            std::fs::read_dir(&sessions).unwrap().count(),
            2,
            "one log and its own sidecar, and nothing from the two deleted"
        );
        assert!(Path::new(&left[0].log).with_extension("json").is_file());
    }

    #[test]
    fn a_huge_log_is_read_from_the_end_with_a_note() {
        let dir = Dir::new("huge");
        let path = dir.0.join("big.log");
        let body: String = (0..500).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();

        let shown = read_log(&path, 200).unwrap();
        assert!(shown.starts_with("(this log is"));
        assert!(shown.trim_end().ends_with("line 499"));
        assert!(!shown.contains("line 100"));
        // A small one comes back whole, with no note in front of it.
        assert_eq!(read_log(&path, 1 << 20).unwrap(), body);
    }
}
