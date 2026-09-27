//! Which file on disk a path actually names.
//!
//! # Why this exists
//!
//! Mods are deployed by **hardlink**: the real bytes live once, in the staging
//! folder, and the mods folder holds second names for them. That is how this
//! program deploys, and it is how Vortex and Mod Organizer deploy too.
//!
//! A hardlink is not a copy and not a shortcut -- it is another name for the
//! same file, and the file system knows it. On NTFS every file has a
//! `(volume serial, file index)` pair that is the same through every one of its
//! names and different for every other file, including a byte-for-byte copy.
//!
//! That makes it the one piece of evidence about "where did this mod come
//! from" that no mod manager has to be asked for and none can be wrong about.
//! [`adopt`](super::adopt) uses it to rebuild the loadout from the disk itself,
//! which is what lets someone who has never run Vortex take over a mods folder
//! that some other program deployed.
//!
//! # Why not `std`
//!
//! `std::os::windows::fs::MetadataExt::file_index` is exactly this and has been
//! unstable since 2019 (rust#63010), so on stable it is `GetFileInformationByHandle`
//! or nothing. The Unix half *is* stable, and is the same idea spelled
//! `(st_dev, st_ino)`.

use std::path::Path;

/// A file, as the file system identifies it -- not a path to one.
///
/// Two paths with the same `FileId` are names for the same bytes on disk. Two
/// files holding identical content have different ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId {
    /// which volume, so ids from two drives cannot be confused
    pub volume: u64,
    /// the file's own number within that volume
    pub index: u64,
}

/// Identify the file at `path`, or `None` if it cannot be opened.
///
/// `None` is an ordinary answer, not an error: a path can be a directory, a
/// broken link, or a file something else has locked. Callers treat it as "this
/// one tells us nothing" and carry on with the rest.
#[cfg(windows)]
pub fn of(path: &Path) -> Option<FileId> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call; the handle is
    // closed on every path out.
    unsafe {
        // Every share mode, because this only reads the file's identity and
        // must not stop the game -- or anything else -- from using it while we
        // do. Opening for read alone would fail on a file another program has
        // open for writing, which during a deploy is a real possibility.
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let read = GetFileInformationByHandle(handle, &mut info).is_ok();
        let _ = CloseHandle(handle);
        if !read {
            return None;
        }
        Some(FileId {
            volume: info.dwVolumeSerialNumber as u64,
            index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        })
    }
}

#[cfg(not(windows))]
pub fn of(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(FileId {
        volume: meta.dev(),
        index: meta.ino(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Dir(PathBuf);

    impl Dir {
        /// On the crate's own volume, deliberately: a hardlink cannot cross
        /// drives, and the temp folder is not always on the same one.
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/fileid_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The whole premise: a deployed name and its staged original are one file.
    #[test]
    fn a_hardlink_is_the_same_file_as_its_original() {
        let dir = Dir::new("link");
        let staged = dir.0.join("staged.MBIN");
        let deployed = dir.0.join("deployed.MBIN");
        std::fs::write(&staged, b"some mod bytes").unwrap();
        std::fs::hard_link(&staged, &deployed).unwrap();

        let a = of(&staged).expect("the original is identifiable");
        let b = of(&deployed).expect("so is the link");
        assert_eq!(a, b, "a hardlink names the same file");
    }

    /// And the part that makes it evidence rather than a guess: identical
    /// content is not the same file. Two mods shipping the same vanilla asset
    /// must not be mistaken for one another.
    #[test]
    fn identical_content_is_not_the_same_file() {
        let dir = Dir::new("copy");
        let one = dir.0.join("one.MBIN");
        let two = dir.0.join("two.MBIN");
        std::fs::write(&one, b"identical").unwrap();
        std::fs::write(&two, b"identical").unwrap();

        let a = of(&one).expect("identifiable");
        let b = of(&two).expect("identifiable");
        assert_ne!(a, b, "same bytes, different files");
    }

    /// A path that is not there is not an error, it is simply no answer.
    #[test]
    fn a_missing_file_has_no_identity() {
        let dir = Dir::new("gone");
        assert_eq!(of(&dir.0.join("not-here.MBIN")), None);
    }
}
