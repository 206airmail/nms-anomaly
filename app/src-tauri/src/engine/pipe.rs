//! The pipe the hook inside the game talks down.
//!
//! One name, one instance, one direction: the hook connects as a client and
//! writes UTF-8 JSON lines, we are the server and only read. That asymmetry is
//! the whole security story of this feature -- nothing the game or a mod sends
//! can make this program do anything except write a line in a log.
//!
//! **This program is the server even though the game starts first.** The hook
//! retries the connection every second for as long as the game runs and replays
//! everything it buffered when it gets through, so a session that begins before
//! the app is opened is still recorded in full. The alternative -- the game
//! hosting -- would lose everything written before we asked.
//!
//! **One instance is deliberate.** Two readers of one hook would each get half
//! the lines. When the name is already taken, [`Server::open`] says so plainly:
//! on this machine that used to mean the standalone NMS Logger was still
//! running, which is exactly the thing the user needs told.
//!
//! Blocking calls on a thread of their own, rather than overlapped I/O: the
//! whole job is "wait for a connection, then read until it ends", and
//! [`nudge`] -- a client connection we make and immediately drop -- is enough
//! to get a waiting server moving when we want to stop.

/// The name the hook is built to connect to. Changing it means rebuilding the
/// DLL (`hook/src/common.h`), so it is fixed here on purpose.
pub const PIPE_NAME: &str = r"\\.\pipe\NMSLogger";

/// How much is read at a time. The hook writes in bursts at startup -- a couple
/// of hundred file opens in the first second -- and this is comfortably more
/// than one burst.
const CHUNK: usize = 16 * 1024;

/// A line longer than this is not a line. Nothing the hook sends approaches it;
/// the cap is here so a truncated or hostile producer cannot grow this process
/// without limit.
const LONGEST_LINE: usize = 1 << 20;

/// What ended a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The game closed the pipe: the session is over.
    Closed,
    /// The caller asked to stop.
    Asked,
}

#[cfg(windows)]
pub use windows_impl::{nudge, Server};

#[cfg(windows)]
mod windows_impl {
    use super::{Ended, CHUNK, LONGEST_LINE, PIPE_NAME};

    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, ERROR_PIPE_CONNECTED, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_NONE,
        OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    /// Our end of the pipe: created once, reused for session after session.
    pub struct Server {
        handle: HANDLE,
    }

    // SAFETY: a pipe handle is a kernel object index with no thread affinity.
    // Only one thread ever reads it -- the recorder's -- and `nudge` touches a
    // separate client handle of its own.
    unsafe impl Send for Server {}

    impl Server {
        /// Create the pipe, or say why it could not be created.
        pub fn open() -> Result<Server, String> {
            Server::open_named(PIPE_NAME)
        }

        /// The same, under a name of your choosing.
        ///
        /// Only the hook's name matters in the app, and it is fixed. This exists
        /// so the plumbing can be exercised end to end -- see
        /// `examples/session_dump.rs` -- without taking the one pipe instance
        /// away from whatever is recording the real game at the time.
        pub fn open_named(pipe: &str) -> Result<Server, String> {
            let name = HSTRING::from(pipe);
            let handle = unsafe {
                CreateNamedPipeW(
                    &name,
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    0,
                    CHUNK as u32 * 4,
                    0,
                    None,
                )
            };
            // This call reports failure with an invalid handle rather than with
            // an error, so the reason has to be fetched separately.
            if handle.is_invalid() {
                let err = windows::core::Error::from_win32();
                return Err(format!(
                    "could not open {PIPE_NAME}: {}. Something else may already be recording \
                     -- the standalone NMS Logger, or another copy of this program.",
                    err.message()
                ));
            }
            Ok(Server { handle })
        }

        /// Wait for the game's hook to connect.
        ///
        /// Returns once a client is on the other end. [`nudge`] also satisfies
        /// it, which is how a waiting thread is asked to stop -- the caller sees
        /// a connection that then reads nothing.
        pub fn wait_for_client(&self) -> Result<(), String> {
            let joined = unsafe { ConnectNamedPipe(self.handle, None) };
            match joined {
                Ok(()) => Ok(()),
                Err(err) if err.code() == ERROR_PIPE_CONNECTED.to_hresult() => {
                    // A client that arrived between creating the pipe and this
                    // call is already connected. Not an error.
                    Ok(())
                }
                Err(err) => Err(format!("waiting on {PIPE_NAME} failed: {}", err.message())),
            }
        }

        /// Read lines until the game goes away, or `keep_going` says stop.
        ///
        /// The producer writes bytes, not lines: one read can hold half a line,
        /// forty lines, or a multi-line crash report, so lines are assembled
        /// here rather than assumed.
        pub fn read_lines(
            &self,
            mut take: impl FnMut(&str),
            keep_going: impl Fn() -> bool,
        ) -> Result<Ended, String> {
            let mut buffer = vec![0u8; CHUNK];
            let mut held: Vec<u8> = Vec::with_capacity(CHUNK);

            loop {
                if !keep_going() {
                    return Ok(Ended::Asked);
                }
                let mut read = 0u32;
                let got = unsafe { ReadFile(self.handle, Some(&mut buffer), Some(&mut read), None) };
                match got {
                    Ok(()) if read == 0 => {
                        // Zero bytes on a byte-mode pipe means the writer has
                        // gone. A `nudge` connection lands here too.
                        break;
                    }
                    Ok(()) => {}
                    Err(err) if err.code() == ERROR_BROKEN_PIPE.to_hresult() => break,
                    Err(err) => {
                        return Err(format!("reading {PIPE_NAME} failed: {}", err.message()))
                    }
                }

                held.extend_from_slice(&buffer[..read as usize]);
                let mut from = 0;
                while let Some(at) = held[from..].iter().position(|b| *b == b'\n') {
                    let line = &held[from..from + at];
                    let line = match line.last() {
                        Some(b'\r') => &line[..line.len() - 1],
                        _ => line,
                    };
                    take(&String::from_utf8_lossy(line));
                    from += at + 1;
                }
                held.drain(..from);

                if held.len() > LONGEST_LINE {
                    // Not a line by any reading. Hand it over as it stands so
                    // it is visible, and start again.
                    take(&String::from_utf8_lossy(&held));
                    held.clear();
                }
            }

            // Whatever was in flight when the pipe closed is still worth having:
            // a crash report is the last thing sent and the newline after it may
            // never have been written.
            if !held.is_empty() {
                take(&String::from_utf8_lossy(&held));
            }
            Ok(Ended::Closed)
        }

        /// Let go of this client, so the next game run can connect.
        pub fn disconnect(&self) {
            unsafe {
                let _ = DisconnectNamedPipe(self.handle);
            }
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            unsafe {
                let _ = DisconnectNamedPipe(self.handle);
                let _ = CloseHandle(self.handle);
            }
        }
    }

    /// Get a thread that is waiting for a client moving again.
    ///
    /// Connects to our own pipe and closes it at once. The waiting server sees a
    /// client that sends nothing and hangs up, which is enough for it to look at
    /// its stop flag -- and if a real game connects in the same instant, nothing
    /// is lost: the hook reconnects every second and replays its backlog.
    pub fn nudge() {
        let name = HSTRING::from(PIPE_NAME);
        unsafe {
            if let Ok(handle) = CreateFileW(
                &name,
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) {
                let _ = CloseHandle(handle);
            }
        }
    }
}

#[cfg(not(windows))]
pub use elsewhere::{nudge, Server};

/// No game, no pipe. Present so the crate still builds and tests elsewhere.
#[cfg(not(windows))]
mod elsewhere {
    use super::Ended;

    pub struct Server;

    impl Server {
        pub fn open() -> Result<Server, String> {
            Err("recording a session needs Windows, where the game is".to_string())
        }

        pub fn open_named(_pipe: &str) -> Result<Server, String> {
            Server::open()
        }

        pub fn wait_for_client(&self) -> Result<(), String> {
            Err("recording a session needs Windows".to_string())
        }

        pub fn read_lines(
            &self,
            _take: impl FnMut(&str),
            _keep_going: impl Fn() -> bool,
        ) -> Result<Ended, String> {
            Err("recording a session needs Windows".to_string())
        }

        pub fn disconnect(&self) {}
    }

    pub fn nudge() {}
}
