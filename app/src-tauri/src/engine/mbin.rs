//! Reader for the 32-byte header in front of every compiled `.MBIN`.
//!
//! Layout confirmed against all 1870 MBIN files in a real mod library:
//!
//! | offset | size | meaning |
//! |--------|------|---------|
//! | `0x00` | 8 | magic, always `CC CC CC CC CC CC CC CC` |
//! | `0x08` | 4 | format word, `0x00040CE4` once MBINCompiler has written the file, `0x00000CE4` for an untouched vanilla extract |
//! | `0x0C` | 4 | low half of the template GUID |
//! | `0x10` | 8 | remainder of the GUID / build timestamp |
//! | `0x18` | 4 | MBINCompiler version: one byte each of major, minor, patch, build |
//!
//! The version stamp is what makes staleness detection possible for binary
//! assets: the byte pair `06 1E` decodes to `6.30` and matches the
//! `<!--File created using MBINCompiler version (6.30.0.1)-->` comment the
//! same tool writes into the EXML form of the asset.
//!
//! Port of `nmscc/mbin.py`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::version::Version;

pub const MBIN_MAGIC: u64 = 0xCCCC_CCCC_CCCC_CCCC;
pub const HEADER_SIZE: usize = 0x20;

/// Format word written by MBINCompiler; vanilla extracts lack the `0x0004`.
pub const FORMAT_COMPILED: u32 = 0x0004_0CE4;

#[derive(Debug, Clone, Default)]
pub struct MbinHeader {
    pub valid: bool,
    pub format_word: u32,
    pub guid: u64,
    pub version: Option<Version>,
    pub error: Option<String>,
}

impl MbinHeader {
    fn invalid(error: String) -> Self {
        Self {
            valid: false,
            error: Some(error),
            ..Default::default()
        }
    }

    /// True when MBINCompiler recorded a version in this file.
    pub fn stamped(&self) -> bool {
        self.version.is_some()
    }
}

/// Parse the header of the MBIN at `path`.
///
/// Never returns an error: an unreadable or malformed file is reported as an
/// invalid header with a reason, exactly as the Python does, so one bad file
/// in a library cannot abort a scan.
pub fn read_header(path: &Path) -> MbinHeader {
    let mut blob = [0u8; HEADER_SIZE];
    let read = match File::open(path).and_then(|mut fh| fh.read(&mut blob)) {
        Ok(n) => n,
        Err(exc) => return MbinHeader::invalid(exc.to_string()),
    };

    if read < HEADER_SIZE {
        return MbinHeader::invalid("file shorter than MBIN header".to_string());
    }

    let magic = u64::from_le_bytes(blob[0..8].try_into().unwrap());
    if magic != MBIN_MAGIC {
        return MbinHeader::invalid(format!("bad magic 0x{magic:016X}"));
    }

    let format_word = u32::from_le_bytes(blob[0x08..0x0C].try_into().unwrap());
    let guid = u64::from_le_bytes(blob[0x0C..0x14].try_into().unwrap()) & 0xFFFF_FFFF_FFFF;

    let (major, minor, patch, build) = (blob[0x18], blob[0x19], blob[0x1A], blob[0x1B]);
    // A zero major means the field was never written, not version 0.
    let version = (major != 0).then(|| {
        Version::new(major as u32, minor as u32, patch as u32, build as u32)
    });

    MbinHeader {
        valid: true,
        format_word,
        guid,
        version,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(bytes: &[u8]) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nmscheck-mbin-test-{}-{:?}.MBIN",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

    fn header_with(version: [u8; 4]) -> Vec<u8> {
        let mut blob = vec![0xCCu8; 8];
        blob.extend_from_slice(&FORMAT_COMPILED.to_le_bytes());
        blob.extend_from_slice(&[0u8; 12]); // guid area, 0x0C..0x18
        blob.extend_from_slice(&version);
        blob.resize(HEADER_SIZE, 0);
        blob
    }

    #[test]
    fn reads_the_version_stamp() {
        let path = write_temp(&header_with([6, 30, 0, 1]));
        let header = read_header(&path);
        std::fs::remove_file(&path).ok();

        assert!(header.valid, "{:?}", header.error);
        assert_eq!(header.version.unwrap().to_string(), "6.30.0.1");
        assert_eq!(header.format_word, FORMAT_COMPILED);
    }

    #[test]
    fn an_unstamped_file_is_valid_but_unversioned() {
        let path = write_temp(&header_with([0, 0, 0, 0]));
        let header = read_header(&path);
        std::fs::remove_file(&path).ok();

        assert!(header.valid);
        assert!(!header.stamped());
    }

    #[test]
    fn bad_magic_is_reported_not_panicked() {
        let mut blob = header_with([7, 3, 2, 2]);
        blob[0] = 0x00;
        let path = write_temp(&blob);
        let header = read_header(&path);
        std::fs::remove_file(&path).ok();

        assert!(!header.valid);
        assert!(header.error.unwrap().starts_with("bad magic"));
    }

    #[test]
    fn a_truncated_file_is_reported_not_panicked() {
        let path = write_temp(&[0xCC; 8]);
        let header = read_header(&path);
        std::fs::remove_file(&path).ok();

        assert!(!header.valid);
        assert_eq!(header.error.unwrap(), "file shorter than MBIN header");
    }

    #[test]
    fn a_missing_file_is_reported_not_panicked() {
        let header = read_header(Path::new("no-such-file-here.MBIN"));
        assert!(!header.valid);
        assert!(header.error.is_some());
    }
}
