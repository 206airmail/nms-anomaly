//! Whether the game is running, and how it stopped.
//!
//! The recorder needs three answers Windows alone can give: is `NMS.exe` there
//! now, hold on to it so it cannot be missed when it goes, and what its exit
//! code was afterwards. The third is the reason a handle is opened the moment
//! the game is noticed rather than when it disappears -- once the last handle to
//! a process closes, the exit code is gone, and "it crashed with an access
//! violation" would degrade to "it stopped".
//!
//! Two things this deliberately does *not* do. It does not kill or suspend
//! anything: the only calls here are read-only, asked with
//! `PROCESS_QUERY_LIMITED_INFORMATION`, the least right that answers the
//! question. And it never treats "the game is running" as a reason to act on
//! the game's files -- that check belongs to [`super::hook`], which must not
//! write a DLL into a folder the loader has open.

use std::time::{Duration, Instant};

/// The game's process name, as Windows reports it.
pub const GAME_EXE: &str = "NMS.exe";

/// A running No Man's Sky, held open so its exit code survives.
///
/// Dropping this closes the handle. Nothing else here owns one, so a session
/// that ends without reading the code simply loses it, which is the honest
/// outcome rather than a leak.
pub struct Running {
    pub pid: u32,
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

// SAFETY: a process handle is just a kernel object index. It has no thread
// affinity, and every call made on it here is read-only.
#[cfg(windows)]
unsafe impl Send for Running {}

/// The pid of the running game, or `None` when it is not running.
#[cfg(windows)]
pub fn find() -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if name.eq_ignore_ascii_case(GAME_EXE) {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
pub fn find() -> Option<u32> {
    None
}

/// Hold a running game open, so its exit code can be read after it goes.
#[cfg(windows)]
pub fn open(pid: u32) -> Option<Running> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `SYNCHRONIZE`, spelled out: the standard right that allows a wait. It is
    /// declared in this crate as a *file* access right, and pasting that type
    /// into a process mask would be a lie about what it is.
    const CAN_WAIT_ON_IT: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(0x0010_0000);

    let handle = unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | CAN_WAIT_ON_IT, false, pid)
    }
    .ok()?;
    Some(Running { pid, handle })
}

#[cfg(not(windows))]
pub fn open(_pid: u32) -> Option<Running> {
    None
}

impl Running {
    /// Wait for the game to stop, and give its exit code.
    ///
    /// `None` means it was still running when the wait ran out -- which happens
    /// for real: the hook's pipe closes on `DLL_PROCESS_DETACH`, and a process
    /// that is winding down can take a moment after that, while one that has
    /// hung can take forever.
    #[cfg(windows)]
    pub fn wait_for_exit(&self, patience: Duration) -> Option<i32> {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::WaitForSingleObject;

        let ms = patience.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        let ended = unsafe { WaitForSingleObject(self.handle, ms) };
        if ended != WAIT_OBJECT_0 {
            return None;
        }
        self.exit_code()
    }

    #[cfg(not(windows))]
    pub fn wait_for_exit(&self, _patience: Duration) -> Option<i32> {
        None
    }

    /// The exit code, or `None` while it is still running.
    #[cfg(windows)]
    pub fn exit_code(&self) -> Option<i32> {
        use windows::Win32::System::Threading::GetExitCodeProcess;

        /// Windows' "not finished yet" sentinel. A process really can exit with
        /// 259, and then this reads it as still running for as long as the
        /// handle is open -- there is no way to tell the two apart through this
        /// API. Nothing exits 259 in practice; the alternative is a job object
        /// per session, which is a lot of machinery for a wrong number.
        const STILL_ACTIVE: u32 = 259;

        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.handle, &mut code) }.ok()?;
        (code != STILL_ACTIVE).then_some(code as i32)
    }

    #[cfg(not(windows))]
    pub fn exit_code(&self) -> Option<i32> {
        None
    }

    /// True while the game is still there.
    pub fn alive(&self) -> bool {
        self.exit_code().is_none()
    }
}

#[cfg(windows)]
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Wait for the game to appear, checking now and then. `None` if it never does.
pub fn wait_for_start(patience: Duration, every: Duration) -> Option<u32> {
    let until = Instant::now() + patience;
    loop {
        if let Some(pid) = find() {
            return Some(pid);
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(every);
    }
}

/// True when an exit code is an NTSTATUS failure rather than a program's own.
///
/// The top two bits set is the NTSTATUS "error" severity, which is how a
/// process killed by an unhandled exception exits. A game that chose to exit 1
/// did something else, and saying "crash" about it would be wrong.
pub fn is_crash_code(code: i32) -> bool {
    (code as u32) & 0xC000_0000 == 0xC000_0000
}

/// What an exit code means, in words rather than in hex.
pub fn describe_exit(code: i32) -> String {
    let named = match code as u32 {
        0 => "normal exit",
        0xC000_0005 => "crash: access violation (read or write through a bad pointer)",
        0xC000_00FD => "crash: stack overflow",
        0xC000_0374 => "crash: heap corruption",
        0xC000_0409 => "crash: fail-fast, often a deliberate abort on a broken invariant",
        0xC000_001D => "crash: illegal instruction",
        0xC000_0094 => "crash: integer divide by zero",
        0xC000_0006 => "crash: in-page error, a file or disk read failed",
        0xC000_0017 => "crash: out of memory",
        0x4001_0004 => "killed, by a debugger or by Task Manager",
        1 => "exited with error code 1",
        _ if is_crash_code(code) => "crash: unhandled exception",
        _ => "exited with a non-zero code",
    };
    named.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_exit_is_not_a_crash() {
        assert!(!is_crash_code(0));
        assert_eq!(describe_exit(0), "normal exit");
    }

    #[test]
    fn an_ordinary_error_code_is_not_a_crash() {
        // The trap this guards: the game exiting 1 must not be reported as a
        // crash, because a crash sends the reader looking for a dump file.
        assert!(!is_crash_code(1));
        assert!(!describe_exit(1).contains("crash"));
    }

    #[test]
    fn an_ntstatus_failure_is_a_crash_even_when_it_is_not_one_we_name() {
        assert!(is_crash_code(0xC000_0005u32 as i32));
        assert!(describe_exit(0xC000_0005u32 as i32).contains("access violation"));
        assert!(is_crash_code(0xC000_0BADu32 as i32));
        assert!(describe_exit(0xC000_0BADu32 as i32).contains("crash"));
    }

    #[test]
    fn a_process_that_is_not_there_cannot_be_opened() {
        // pid 0 is the system idle process; it is never something we can hold.
        assert!(open(0).is_none());
    }
}
