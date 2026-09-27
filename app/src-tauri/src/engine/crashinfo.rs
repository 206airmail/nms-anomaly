//! Evidence about a crash that the hook inside the game could not write.
//!
//! When the game dies badly, the in-process crash report is the best account of
//! it -- module, offset, registers, the files it had just opened. But a process
//! killed hard enough writes nothing at all, and *that* is exactly the case
//! where the reader most needs something. Windows keeps two records of it
//! regardless:
//!
//! - the Application event log, where Windows Error Reporting files the faulting
//!   module and exception code;
//! - a dump file, written by the game's own handler or by WER.
//!
//! Both are collected after the process is gone, which is why the recorder waits
//! a few seconds on a crash before finishing a session: WER files its event
//! *after* the process it is reporting on has exited, so asking immediately
//! reliably finds nothing.
//!
//! **The event log is read through `wevtutil`, not through the Win32 event API.**
//! `EvtQuery`/`EvtNext`/`EvtRender` is three unsafe calls and a rendering buffer
//! to produce XML this project can already parse with `roxmltree`, and it runs
//! at most once per game session. The tool is part of Windows, so there is
//! nothing to ship.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::clock;

/// One Windows record of a program dying.
#[derive(Debug, Clone, Serialize)]
pub struct WinEvent {
    /// local time, as the log prints everything else
    pub at: String,
    pub provider: String,
    pub id: u32,
    /// the fields that matter, in one line
    pub summary: String,
}

/// What Windows knew about the session after the game was gone.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Aftermath {
    pub events: Vec<WinEvent>,
    /// dump files written during this session, newest last
    pub dumps: Vec<String>,
    /// why the event log could not be read, when it could not
    pub note: Option<String>,
}

impl Aftermath {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.dumps.is_empty() && self.note.is_none()
    }
}

/// The event ids Windows files a dead program under.
const IDS: [&str; 3] = ["1000", "1001", "1002"];

/// Collect everything Windows recorded about `about` since `since_ms`.
///
/// `dirs` are the folders to look for dump files in; anything not there is
/// skipped. Five seconds of slack is allowed on the start time because the
/// clock the event log stamps with and the clock this program read are not the
/// same clock, and an event a second early is still this session's.
pub fn collect(about: &str, since_ms: i64, dirs: &[PathBuf]) -> Aftermath {
    let (events, note) = windows_events(about, since_ms - 5_000);
    Aftermath {
        events,
        dumps: dumps(about, since_ms - 5_000, dirs),
        note,
    }
}

/// Ask the Application log what it filed about `about` since then.
fn windows_events(about: &str, since_ms: i64) -> (Vec<WinEvent>, Option<String>) {
    let at = clock::utc(since_ms);
    let since = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    );
    let query = format!(
        "*[System[({}) and TimeCreated[@SystemTime>='{since}']]]",
        IDS.iter()
            .map(|id| format!("EventID={id}"))
            .collect::<Vec<_>>()
            .join(" or ")
    );

    let mut command = std::process::Command::new("wevtutil");
    command
        .arg("qe")
        .arg("Application")
        .arg(format!("/q:{query}"))
        .arg("/f:xml")
        // Newest first, and few enough that a machine with a busy log does not
        // hand back megabytes. A game crashes once per session.
        .arg("/c:20")
        .arg("/rd:true")
        // Without this the output is a stream of sibling elements with no root,
        // which is not a document any parser will accept.
        .arg("/e:Events");
    no_window(&mut command);

    let out = match command.output() {
        Ok(out) => out,
        Err(err) => return (Vec::new(), Some(format!("could not run wevtutil: {err}"))),
    };
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return (
            Vec::new(),
            Some(format!(
                "could not read the Windows Application log{}",
                if said.is_empty() {
                    String::new()
                } else {
                    format!(": {said}")
                }
            )),
        );
    }

    let text = String::from_utf8_lossy(&out.stdout).to_string();
    match parse_events(&text, about) {
        Ok(found) => (found, None),
        Err(err) => (Vec::new(), Some(err)),
    }
}

/// Pull the events that name `about` out of `wevtutil`'s XML.
///
/// Separate from the call so it can be tested against a captured document:
/// there is no way to make Windows file a crash on demand.
fn parse_events(xml: &str, about: &str) -> Result<Vec<WinEvent>, String> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|err| format!("the Windows event log came back in a shape we cannot read: {err}"))?;

    let mut found = Vec::new();
    for event in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Event")
    {
        // Every field is optional as far as this code is concerned: a record
        // missing one is still worth reporting, with a gap where it was.
        let system = event
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "System");
        let field = |name: &str| {
            system.and_then(|s| {
                s.children()
                    .find(|n| n.is_element() && n.tag_name().name() == name)
            })
        };

        let provider = field("Provider")
            .and_then(|n| n.attribute("Name"))
            .unwrap_or("Windows")
            .to_string();
        let id = field("EventID")
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let at = field("TimeCreated")
            .and_then(|n| n.attribute("SystemTime"))
            .map(local_from_iso)
            .unwrap_or_else(|| "?".to_string());

        // `Data` elements are named in the modern schema and bare in the old
        // one, so both are read: name=value where there is a name, the value
        // alone where there is not.
        let mut parts: Vec<String> = Vec::new();
        let mut mentions = false;
        for data in event.descendants().filter(|n| n.is_element() && n.tag_name().name() == "Data") {
            let value = data.text().unwrap_or("").trim().to_string();
            if value.is_empty() {
                continue;
            }
            if value.to_lowercase().contains(&about.to_lowercase()) {
                mentions = true;
            }
            match data.attribute("Name") {
                Some(name) => parts.push(format!("{name}={value}")),
                None => parts.push(value),
            }
        }
        if !mentions {
            continue;
        }
        found.push(WinEvent {
            at,
            provider,
            id,
            summary: parts.join(", "),
        });
    }
    Ok(found)
}

/// `2026-09-25T13:37:48.3009341Z` as a local clock reading.
///
/// Only the fields are parsed, not a general ISO-8601 grammar: this string
/// comes from one producer, in one shape, and a value that does not match it is
/// shown as it arrived rather than guessed at.
fn local_from_iso(stamp: &str) -> String {
    let parse = |slice: &str| slice.parse::<i64>().ok();
    let bytes: Vec<char> = stamp.chars().collect();
    if bytes.len() < 19 || bytes.get(10) != Some(&'T') {
        return stamp.to_string();
    }
    let s: String = bytes.iter().collect();
    let (year, month, day) = (
        parse(&s[0..4]),
        parse(&s[5..7]),
        parse(&s[8..10]),
    );
    let (hour, minute, second) = (parse(&s[11..13]), parse(&s[14..16]), parse(&s[17..19]));
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (year, month, day, hour, minute, second)
    else {
        return stamp.to_string();
    };

    // Days from the civil date, Hinnant's algorithm forwards.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 86_400) + hour * 3_600 + minute * 60 + second) * 1_000;
    clock::local(ms).written()
}

/// Dump files written during this session, wherever they land.
fn dumps(about: &str, since_ms: i64, dirs: &[PathBuf]) -> Vec<String> {
    let stem = about
        .rsplit('.')
        .nth(1)
        .unwrap_or(about)
        .to_lowercase();
    let mut found: Vec<(i64, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for dir in dirs {
        if !dir.is_dir() || !seen.insert(dir.to_string_lossy().to_lowercase()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".dmp") || !name.starts_with(&stem) {
                continue;
            }
            let Some(written) = modified_ms(&entry.path()) else {
                continue;
            };
            if written >= since_ms {
                found.push((written, entry.path().display().to_string()));
            }
        }
    }
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

fn modified_ms(path: &Path) -> Option<i64> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        at.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    )
}

/// The last `lines` lines of a text file, for the hook's own raw log.
///
/// Used when the game crashed and no report came down the pipe: the DLL writes
/// to its file first and to the pipe second, so what the pipe never delivered
/// is often sitting in that file.
pub fn tail(path: &Path, lines: usize) -> Vec<String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..]
                .iter()
                .map(|line| line.trim_end().to_string())
                .collect()
        }
        Err(err) => vec![format!("(could not read {}: {err})", path.display())],
    }
}

/// The usual places a No Man's Sky dump lands.
pub fn dump_dirs(game_root: &Path, bin_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        bin_dir.to_path_buf(),
        game_root.to_path_buf(),
        game_root.join("GAMEDATA"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("CrashDumps"));
    }
    if let Ok(roaming) = std::env::var("APPDATA") {
        dirs.push(PathBuf::from(roaming).join("HelloGames").join("NMS"));
    }
    dirs.push(std::env::temp_dir());
    dirs
}

/// Keep a shelled-out console tool from flashing a window over the game.
#[cfg(windows)]
fn no_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    /// CREATE_NO_WINDOW
    const QUIET: u32 = 0x0800_0000;
    command.creation_flags(QUIET);
}

#[cfg(not(windows))]
fn no_window(_command: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// One real record, trimmed: Application Error for a faulting program.
    const ONE: &str = r#"<Events>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>
<Provider Name='Application Error'/><EventID>1000</EventID>
<TimeCreated SystemTime='2026-09-25T13:37:48.3009341Z'/></System>
<EventData><Data Name='AppName'>NMS.exe</Data><Data Name='ModuleName'>NMS.exe</Data>
<Data Name='ExceptionCode'>c0000005</Data><Data Name='AppPath'>D:\SteamLibrary\steamapps\common\No Man's Sky\Binaries\NMS.exe</Data>
<Data Name='PackageFullName'></Data></EventData></Event>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>
<Provider Name='Application Error'/><EventID>1000</EventID>
<TimeCreated SystemTime='2026-09-16T20:41:30.5484373Z'/></System>
<EventData><Data Name='AppName'>tbs_browser.exe</Data><Data Name='ExceptionCode'>80000003</Data></EventData></Event>
</Events>"#;

    #[test]
    fn only_the_events_naming_the_game_are_kept() {
        let found = parse_events(ONE, "NMS.exe").unwrap();
        assert_eq!(found.len(), 1, "the other program's crash is not ours to report");
        assert_eq!(found[0].id, 1000);
        assert_eq!(found[0].provider, "Application Error");
        assert!(found[0].summary.contains("ExceptionCode=c0000005"));
    }

    #[test]
    fn an_empty_field_is_left_out_rather_than_printed_as_a_gap() {
        let found = parse_events(ONE, "NMS.exe").unwrap();
        assert!(!found[0].summary.contains("PackageFullName"));
    }

    #[test]
    fn a_document_we_cannot_parse_is_a_note_not_a_panic() {
        let err = parse_events("<Events><Event>", "NMS.exe").unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    #[test]
    fn an_event_time_is_shown_on_the_readers_clock() {
        let found = parse_events(ONE, "NMS.exe").unwrap();
        // Whatever zone this machine is in, the shape is the one the log header
        // uses, not the raw ISO string.
        assert!(
            found[0].at.starts_with("2026-09-2") && found[0].at.contains(':'),
            "{}",
            found[0].at
        );
    }

    #[test]
    fn an_unparseable_timestamp_is_shown_as_it_arrived() {
        assert_eq!(local_from_iso("not a time"), "not a time");
    }

    #[test]
    fn only_dumps_written_during_the_session_are_collected() {
        let dir = std::env::temp_dir().join("nmscheck_crashinfo_dumps");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("NMS_crash_1.dmp"), b"dump").unwrap();
        std::fs::write(dir.join("something_else.dmp"), b"dump").unwrap();

        let now = clock::now_ms();
        let mine = dumps("NMS.exe", now - 60_000, &[dir.clone()]);
        assert_eq!(mine.len(), 1, "{mine:?}");
        assert!(mine[0].ends_with("NMS_crash_1.dmp"));

        // A session that started after the file was written must not claim it.
        assert!(dumps("NMS.exe", now + 60_000, &[dir.clone()]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tail_of_a_missing_file_says_so_rather_than_being_silent() {
        let said = tail(Path::new(r"Z:\nope\hook_latest.log"), 5);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("could not read"));
    }

    #[test]
    fn the_tail_is_the_end_of_the_file() {
        let path = std::env::temp_dir().join("nmscheck_crashinfo_tail.log");
        std::fs::write(&path, "\u{feff}one\r\ntwo\r\nthree\r\nfour\r\n").unwrap();
        assert_eq!(tail(&path, 2), vec!["three".to_string(), "four".to_string()]);
        // Asking for more than there is gives everything, not a panic.
        assert_eq!(tail(&path, 99).len(), 4);
        let _ = std::fs::remove_file(&path);
    }
}
