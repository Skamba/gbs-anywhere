//! Wall-clock times as the app shows them: UTC in RFC 3339, without a
//! date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// `2026-10-11T12:08:01.663Z`.
pub(crate) fn with_millis(t: SystemTime) -> String {
    let millis = t
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis();
    format!("{}.{millis:03}Z", format(t))
}

/// `2026-10-11T12:08:01Z`.
pub(crate) fn with_seconds(t: SystemTime) -> String {
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

/// The seconds since 1970 of a time as [`with_seconds`] or [`with_millis`]
/// write it (the milliseconds left out), or `None` for anything else.
pub(crate) fn seconds(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let text = text.split_once('.').map_or(text, |(whole, _)| whole);
    let (date, time) = text.split_once('T')?;
    let numbers = |s: &str, n: usize| -> Option<Vec<u64>> {
        let parts: Vec<u64> = s
            .split(['-', ':'])
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        (parts.len() == n).then_some(parts)
    };
    let (d, t) = (numbers(date, 3)?, numbers(time, 3)?);
    let (y, m, day) = (d[0], d[1], d[2]);
    if y < 1970 || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    if t[0] > 23 || t[1] > 59 || t[2] > 59 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + t[0] * 3600 + t[1] * 60 + t[2])
}

/// The days since 1970-01-01 of a date in the Gregorian calendar, from 1970
/// on (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: u64, m: u64, d: u64) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y % 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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

    #[test]
    fn reads_what_it_writes() {
        for secs in [
            0,
            951_782_400,
            951_868_800,
            1_000_000_000,
            1_791_720_481,
            4_102_444_799,
        ] {
            assert_eq!(seconds(&with_seconds(at(secs, 0))), Some(secs), "{secs}");
            assert_eq!(seconds(&with_millis(at(secs, 663))), Some(secs), "{secs}");
        }
        for bad in [
            "",
            "2026-10-11T12:08:01",
            "2026-10-11 12:08:01Z",
            "2026-13-11T12:08:01Z",
            "2026-10-11T24:00:00Z",
            "1969-12-31T23:59:59Z",
            "2026-10-11T12:08Z",
            "x026-10-11T12:08:01Z",
        ] {
            assert_eq!(seconds(bad), None, "{bad:?}");
        }
    }
}
