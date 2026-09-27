//! MBINCompiler version stamps.
//!
//! Port of the `Version` half of `nmscc/model.py`. The string form matters:
//! it goes straight into the JSON the UI reads, so it has to match the Python
//! `f"{major}.{minor:02d}.{patch}.{build}"` exactly, zero padding included.

use std::cmp::Ordering;
use std::fmt;

/// A stamp such as `6.34.0.3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub build: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32, build: u32) -> Self {
        Self {
            major,
            minor,
            patch,
            build,
        }
    }

    /// Sort key, worst-to-best ordering handled by `Ord`.
    pub fn key(&self) -> (u32, u32, u32, u32) {
        (self.major, self.minor, self.patch, self.build)
    }

    /// The NMS game version the stamp corresponds to, e.g. `6.34`.
    pub fn game(&self) -> String {
        format!("{}.{:02}", self.major, self.minor)
    }

    /// Parse `"6.34.0.3"`. Returns `None` when the text is unusable.
    ///
    /// Two or more components are required, because a bare major number says
    /// nothing about the game version.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let nums: Option<Vec<u32>> = parts
            .iter()
            .take(4)
            .map(|p| p.trim().parse::<u32>().ok())
            .collect();
        let nums = nums?;
        Some(Self {
            major: nums[0],
            minor: nums[1],
            patch: nums.get(2).copied().unwrap_or(0),
            build: nums.get(3).copied().unwrap_or(0),
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{:02}.{}.{}",
            self.major, self.minor, self.patch, self.build
        )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_is_zero_padded_like_python() {
        assert_eq!(Version::new(7, 3, 2, 2).to_string(), "7.03.2.2");
        assert_eq!(Version::new(6, 34, 0, 3).to_string(), "6.34.0.3");
    }

    #[test]
    fn game_version_drops_patch_and_build() {
        assert_eq!(Version::new(7, 3, 2, 2).game(), "7.03");
        assert_eq!(Version::new(6, 34, 0, 3).game(), "6.34");
    }

    #[test]
    fn parse_round_trips() {
        let v = Version::parse("6.34.0.3").unwrap();
        assert_eq!(v, Version::new(6, 34, 0, 3));
        assert_eq!(v.to_string(), "6.34.0.3");
    }

    #[test]
    fn parse_fills_missing_components() {
        assert_eq!(Version::parse("7.03").unwrap(), Version::new(7, 3, 0, 0));
    }

    #[test]
    fn parse_rejects_unusable_text() {
        assert!(Version::parse("7").is_none());
        assert!(Version::parse("").is_none());
        assert!(Version::parse("six.thirty").is_none());
    }

    #[test]
    fn ordering_is_numeric_not_lexical() {
        // "6.9" must not sort above "6.34" the way string comparison would.
        assert!(Version::parse("6.09").unwrap() < Version::parse("6.34").unwrap());
        assert!(Version::parse("6.34").unwrap() < Version::parse("7.03").unwrap());
    }
}
