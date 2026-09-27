//! Exercise the session recorder end to end, with no game involved.
//!
//! The unit tests cover the parsing, the echo, the attribution and the summary,
//! but not the one part that cannot be unit-tested: the named pipe. This stands
//! in for the game -- it opens the real pipe, connects to it as the hook does,
//! sends the same JSON lines the hook sends, and prints the log and the verdict
//! that come out.
//!
//! Run it with `cargo run --example session_dump`. Nothing is written outside a
//! temporary folder, and nothing is installed into the game.
//!
//! It listens on a pipe name of its own, so it cannot take the single pipe
//! instance away from whatever is recording the real game at the time. Pass a
//! name as an argument to listen on that one instead -- the hook's own name, for
//! instance, to see what the app sees.
//!
//! With `--listen` it sends nothing and records whatever connects, on the hook's
//! own pipe unless a name follows. That is the host side of
//! `hook/test/run_test.ps1`, which runs a fake NMS.exe with the real DLL loaded
//! into it -- the only way to exercise the hook without launching the game.

use std::io::Write;
use std::path::PathBuf;

use anomaly_lib::engine::sessionlog::{About, ModFiles, Recorder};
use anomaly_lib::engine::{pipe, sessionlog};

/// The lines a real session sends, in the order and shapes the hook sends them.
fn script(now: i64) -> Vec<String> {
    let event = |lvl: u8, cat: &str, msg: &str| {
        serde_json::json!({ "ts": now, "lvl": lvl, "tid": 4242, "cat": cat, "msg": msg })
            .to_string()
    };
    vec![
        serde_json::json!({
            "type": "hello",
            "pid": std::process::id(),
            "hookVersion": "0.1.0",
            "exe": r"D:\Game\Binaries\NMS.exe",
            "backlogDropped": 0,
        })
        .to_string(),
        event(0, "hook", "NMS Logger hook 0.1.0 attached to NMS.exe"),
        event(1, "modfile", r"loaded D:\GAME\GAMEDATA\MODS\ALPHA\METADATA\X.MBIN"),
        // The same asset again in another format, which the game never opens.
        event(
            2,
            "modfile",
            r"open failed (error 2): D:\GAME\GAMEDATA\MODS\ALPHA\TEXTURES\GONE.DDS",
        ),
        // The game says everything twice; only one of these should be recorded.
        event(2, "gamelog", "failed to load shader cache"),
        event(2, "debugout", "[NMS.exe] failed to load shader cache"),
        event(
            2,
            "exception",
            "first-chance 0xC0000005 ACCESS_VIOLATION (reading address 0x0) at NMS.exe+0x11E1 (may be handled by the game)",
        ),
        event(3, "dialog", r#"message box "Error": something went wrong"#),
        event(
            4,
            "crash",
            "UNHANDLED EXCEPTION - the game is crashing\n  exception: 0xC0000005 ACCESS_VIOLATION (reading address 0x0)\n  location:  NMS.exe+0x11E1\n  thread:    4242",
        ),
        event(1, "exit", "TerminateProcess on self, exit code 0"),
    ]
}

fn main() {
    let now = anomaly_lib::engine::clock::now_ms();
    let dir = std::env::temp_dir().join("nmscheck_session_dump");
    let _ = std::fs::remove_dir_all(&dir);

    let args: Vec<String> = std::env::args().skip(1).collect();
    // Listening for a real hook means sending no script of our own, and waiting
    // on the pipe the DLL is built to connect to.
    let listening = args.iter().any(|arg| arg == "--listen");
    // Ours by default; the hook's own name when listening, or when named.
    let name = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| {
            if listening {
                pipe::PIPE_NAME.to_string()
            } else {
                r"\\.\pipe\nmscheck-session-dump".to_string()
            }
        });
    println!("pipe     : {name}   (the hook's own is {})", pipe::PIPE_NAME);
    let server = match pipe::Server::open_named(&name) {
        Ok(server) => server,
        Err(err) => {
            println!("\nCOULD NOT LISTEN\n  {err}");
            return;
        }
    };

    if listening {
        println!("waiting for a hook to connect. Ctrl+C to stop.");
    }
    // The hook's side: a separate thread, because connecting and writing while
    // this thread is blocked waiting for a client is exactly the real shape.
    let path = name.clone();
    let sender = (!listening).then(|| std::thread::spawn(move || {
        // A pipe is a file on Windows, which is all the hook needs it to be.
        let mut client = loop {
            match std::fs::OpenOptions::new().write(true).open(&path) {
                Ok(handle) => break handle,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        };
        for line in script(now) {
            let _ = client.write_all(line.as_bytes());
            let _ = client.write_all(b"\n");
        }
        let _ = client.flush();
    }));

    server.wait_for_client().expect("a client to connect");

    let about = About {
        game_root: PathBuf::from(r"D:\Game"),
        bin_dir: PathBuf::from(r"D:\Game\Binaries"),
        mods_dir: PathBuf::from(r"D:\Game\GAMEDATA\MODS"),
        app_version: "example".to_string(),
        machine: None,
        changed: Vec::new(),
    };
    let mods = vec![
        ModFiles {
            folder: "Alpha".into(),
            enabled: true,
            files: vec![
                r"METADATA\X.MBIN".into(),
                r"METADATA\X.MXML".into(),
                r"TEXTURES\GONE.DDS".into(),
            ],
        },
        ModFiles {
            folder: "Beta".into(),
            enabled: true,
            files: vec![r"METADATA\Y.MBIN".into()],
        },
    ];

    let mut recorder: Option<Recorder> = None;
    let mut shown = 0usize;
    server
        .read_lines(
            |line| match recorder.as_mut() {
                None => {
                    let hello = match sessionlog::parse(line) {
                        sessionlog::Line::Hello(hello) => Some(hello),
                        _ => None,
                    };
                    // Listening for a real hook, the greeting says where the game
                    // is -- so the log is written about *that* install rather than
                    // about the made-up one the script uses. That is what makes
                    // the fake-game harness a faithful rehearsal.
                    let (about, mods) = match (listening, hello.as_ref()) {
                        (true, Some(hello)) if !hello.exe.is_empty() => {
                            let exe = PathBuf::from(&hello.exe);
                            let bin_dir = exe.parent().unwrap_or(&exe).to_path_buf();
                            let game_root =
                                bin_dir.parent().unwrap_or(&bin_dir).to_path_buf();
                            let mods_dir = game_root.join("GAMEDATA").join("MODS");
                            let found = sessionlog::inventory(&mods_dir, &game_root);
                            (
                                About {
                                    game_root,
                                    bin_dir,
                                    mods_dir,
                                    app_version: "example".to_string(),
                                    // Not read here: the example is not the
                                    // watcher, and a machine snapshot belongs to
                                    // a session the app itself is recording.
                                    machine: None,
                                    changed: Vec::new(),
                                },
                                found,
                            )
                        }
                        _ => (about.clone(), mods.clone()),
                    };
                    recorder =
                        Some(Recorder::start(&dir, about, mods, hello).expect("a log to open"));
                }
                Some(open) => {
                    if let Some(live) = open.take(line) {
                        shown += 1;
                        println!("  {} {:<9} {}", live.at, live.cat, live.msg.lines().next().unwrap_or(""));
                    }
                }
            },
            || true,
        )
        .expect("to read the stream");
    if let Some(sender) = sender {
        let _ = sender.join();
    }

    let open = recorder.expect("the hello to have opened a log");
    let log = open.log_path().to_path_buf();
    // Scripted: 0xC0000005, so the summary takes the crash path. Listening: we
    // are not the watcher and hold no handle on the game, so its exit code is
    // not ours to claim -- saying "unknown" is the honest answer.
    let brief = if listening {
        open.finish(None, false)
    } else {
        open.finish(Some(0xC000_0005u32 as i32), false)
    };

    println!("\nevents shown : {shown} (one of the two copies of the shader line was dropped)");
    println!("verdict      : {}", brief.verdict);
    println!("counts       : {:?}", brief.counts);
    println!("files opened : {}", brief.files_opened);
    println!("in trouble   : {:?}", brief.mods_in_trouble);
    println!("ignored      : {:?}", brief.ignored);
    println!("never loaded : {:?}", brief.never_loaded);
    println!("\nlog          : {}", log.display());
    println!("{}", "-".repeat(74));
    print!("{}", std::fs::read_to_string(&log).unwrap_or_default());
}
