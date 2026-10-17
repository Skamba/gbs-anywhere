//! Wall-clock times as the app shows them: UTC in RFC 3339, without a
//! date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// `2026-10-11T12:08:01.663Z`.
pub fn with_millis(t: SystemTime) -> String {
    let millis = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis();
    format!("{}.{millis:03}Z", format(t))
}

/// `2026-10-11T12:08:01Z`.
pub fn with_seconds(t: SystemTime) -> String {
    format!("{}Z", format(t))
}

/// `2026-10-11T12:08:01`. Times before 1970 count as 1970.
fn format(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (rest / 3600, rest / 60 % 60, rest % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
}

/// Days since 1970-01-01 as (year, month, day) in the Gregorian calendar
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64, millis: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis)
    }

    #[test]
    fn formats_utc() {
        assert_eq!(with_seconds(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            with_millis(at(1_000_000_000, 7)),
            "2001-09-09T01:46:40.007Z"
        );
        // A leap day, and the day after.
        assert_eq!(with_seconds(at(951_782_400, 0)), "2000-02-29T00:00:00Z");
        assert_eq!(with_seconds(at(951_868_800, 0)), "2000-03-01T00:00:00Z");
        assert_eq!(
            with_millis(at(1_791_720_481, 663)),
            "2026-10-11T12:08:01.663Z"
        );
        assert_eq!(with_seconds(at(4_102_444_799, 0)), "2099-12-31T23:59:59Z");
    }
}
