//! Wall-clock time, in the time zone the reader's clock is in.
//!
//! The session recorder is the first part of this program whose output a person
//! reads *against their own memory* -- "it crashed around quarter past nine" --
//! so every time it prints has to agree with the clock in the corner of their
//! screen. Everything else here stamps folders in UTC, which is right for names
//! that only have to sort.
//!
//! There is no date crate in this project and this is not a reason to add one:
//! the whole job is one civil-date conversion (Howard Hinnant's algorithm, as
//! used in [`super::library`]) and one call into Windows for the zone offset --
//! and Windows is the only platform that has the game.
//!
//! The offset is asked for *per timestamp* rather than once per session. A run
//! of three and a half hours can cross a daylight-saving change, and an hour of
//! events stamped wrong is worse than the few microseconds this costs.

/// A moment, broken out far enough to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub milli: u32,
}

impl Civil {
    /// `2026-09-25_21-12-46` -- sortable, and legal in a path everywhere.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2026-09-25 21:12:46`, for a header a person reads.
    pub fn written(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `21:12:46.123` -- the shape every line of a session log carries.
    pub fn clock(&self) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            self.hour, self.minute, self.second, self.milli
        )
    }
}

/// Unix milliseconds, now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Break out unix milliseconds in the machine's own time zone.
pub fn local(unix_ms: i64) -> Civil {
    utc(unix_ms + offset_ms(unix_ms))
}

/// Break out unix milliseconds as UTC.
pub fn utc(unix_ms: i64) -> Civil {
    let secs = unix_ms.div_euclid(1000);
    let milli = unix_ms.rem_euclid(1000) as u32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);

    // Civil date from a day count, Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    Civil {
        year,
        month: month as u32,
        day: day as u32,
        hour: (rem / 3600) as u32,
        minute: ((rem % 3600) / 60) as u32,
        second: (rem % 60) as u32,
        milli,
    }
}

/// How far ahead of UTC the machine's clock is at that moment, in millis.
///
/// Asked of Windows rather than worked out, because the rules for a zone --
/// which Sunday, at what hour, in which direction -- are data, not arithmetic.
#[cfg(windows)]
fn offset_ms(unix_ms: i64) -> i64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

    // Unix epoch to the FILETIME epoch (1601-01-01), in 100-nanosecond ticks.
    const TO_FILETIME: i64 = 11_644_473_600_000;
    let ticks = (unix_ms + TO_FILETIME).max(0) as u64 * 10_000;
    let file = FILETIME {
        dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };

    let mut as_utc = SYSTEMTIME::default();
    let mut as_local = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&file, &mut as_utc).is_err() {
            return 0;
        }
        if SystemTimeToTzSpecificLocalTime(None, &as_utc, &mut as_local).is_err() {
            return 0;
        }
    }

    // Both are the same instant, so the difference is the offset. Compared as
    // minutes-of-the-era rather than by parsing the dates again: a zone offset
    // is never finer than a minute, and this way a conversion that lands on the
    // other side of midnight or of a month end costs nothing extra.
    (minutes(&as_local) - minutes(&as_utc)) * 60_000
}

#[cfg(windows)]
fn minutes(st: &windows::Win32::Foundation::SYSTEMTIME) -> i64 {
    // Days since an arbitrary fixed point, Hinnant's algorithm run forwards.
    let y = i64::from(st.wYear) - i64::from(st.wMonth <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if st.wMonth > 2 {
        i64::from(st.wMonth) - 3
    } else {
        i64::from(st.wMonth) + 9
    };
    let doy = (153 * mp + 2) / 5 + i64::from(st.wDay) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe;
    days * 1440 + i64::from(st.wHour) * 60 + i64::from(st.wMinute)
}

/// No game, no zone to worry about: the tests and the WSL build land here.
#[cfg(not(windows))]
fn offset_ms(_unix_ms: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_instant_breaks_out_correctly_in_utc() {
        // 2026-09-25T21:12:46.123Z
        let at = utc(1_790_370_766_123);
        assert_eq!(at.year, 2026);
        assert_eq!((at.month, at.day), (9, 25));
        assert_eq!((at.hour, at.minute, at.second), (21, 12, 46));
        assert_eq!(at.milli, 123);
    }

    #[test]
    fn the_epoch_itself_is_not_off_by_a_day() {
        let at = utc(0);
        assert_eq!((at.year, at.month, at.day), (1970, 1, 1));
        assert_eq!((at.hour, at.minute, at.second, at.milli), (0, 0, 0, 0));
    }

    #[test]
    fn a_leap_day_is_a_leap_day() {
        // 2024-02-29T12:00:00Z
        let at = utc(1_709_208_000_000);
        assert_eq!((at.year, at.month, at.day), (2024, 2, 29));
    }

    #[test]
    fn the_printed_shapes_are_the_ones_the_log_and_the_file_names_use() {
        let at = utc(1_790_370_766_123);
        assert_eq!(at.stamp(), "2026-09-25_21-12-46");
        assert_eq!(at.written(), "2026-09-25 21:12:46");
        assert_eq!(at.clock(), "21:12:46.123");
    }

    #[test]
    fn local_time_differs_from_utc_only_by_whole_minutes() {
        // Whatever zone this machine is in, the offset is a whole number of
        // minutes -- so the seconds and millis must survive the conversion.
        let ms = 1_790_370_766_123;
        let there = utc(ms);
        let here = local(ms);
        assert_eq!(here.second, there.second);
        assert_eq!(here.milli, there.milli);
    }

    #[test]
    fn a_millisecond_before_midnight_stays_on_its_own_day() {
        // 2026-01-01T00:00:00Z minus 1ms
        let at = utc(1_767_225_600_000 - 1);
        assert_eq!((at.year, at.month, at.day), (2025, 12, 31));
        assert_eq!((at.hour, at.minute, at.second, at.milli), (23, 59, 59, 999));
    }
}
