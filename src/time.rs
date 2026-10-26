//! Times as Unix nanoseconds, read and written as Go's time package does
//! for RFC 3339: what --updated-since, --since and --if-updated-at take,
//! and what Linear and the demo workspace hold.

use std::time::{SystemTime, UNIX_EPOCH};

pub const SECOND: i64 = 1_000_000_000;
pub const HOUR: i64 = 3600 * SECOND;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_in(m: i64, y: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

struct Parts {
    y: i64,
    mo: i64,
    d: i64,
    h: i64,
    mi: i64,
    s: i64,
    ns: i64,
}

fn parts(t: i64) -> Parts {
    let secs = t.div_euclid(SECOND);
    let ns = t.rem_euclid(SECOND);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let (y, mo, d) = civil_from_days(days);
    Parts { y, mo, d, h: rem / 3600, mi: rem / 60 % 60, s: rem % 60, ns }
}

/// 2006-01-02T15:04:05Z, in UTC.
pub fn rfc3339(t: i64) -> String {
    let p = parts(t);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", p.y, p.mo, p.d, p.h, p.mi, p.s)
}

/// 2006-01-02T15:04:05.000Z, in UTC: Linear's own form.
pub fn rfc3339_ms(t: i64) -> String {
    let p = parts(t);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", p.y, p.mo, p.d, p.h, p.mi, p.s, p.ns / 1_000_000)
}

/// 2006-01-02, in UTC.
pub fn date(t: i64) -> String {
    let p = parts(t);
    format!("{:04}-{:02}-{:02}", p.y, p.mo, p.d)
}

fn digits(s: &[u8], n: usize) -> Option<i64> {
    if s.len() < n || !s[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s[..n].iter().fold(0, |a, &c| a * 10 + i64::from(c - b'0')))
}

/// A date as Go's time.Parse("2006-01-02") reads it: midnight UTC.
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (digits(b, 4)?, digits(&b[5..], 2)?, digits(&b[8..], 2)?);
    if !(1..=12).contains(&mo) || d < 1 || d > days_in(mo, y) {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86400 * SECOND)
}

/// A time as Go's time.Parse(time.RFC3339) reads it: an hour of one or two
/// digits, optional fractional seconds, Z or ±hh:mm.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 11 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let (y, mo, d) = (digits(b, 4)?, digits(&b[5..], 2)?, digits(&b[8..], 2)?);
    let mut i = 11;
    let hlen = if b.len() > i + 1 && b[i + 1].is_ascii_digit() { 2 } else { 1 };
    let h = digits(&b[i..], hlen)?;
    i += hlen;
    if b.get(i) != Some(&b':') {
        return None;
    }
    let mi = digits(&b[i + 1..], 2)?;
    i += 3;
    if b.get(i) != Some(&b':') {
        return None;
    }
    let sec = digits(&b[i + 1..], 2)?;
    i += 3;
    let mut ns = 0;
    if i + 1 < b.len() && (b[i] == b'.' || b[i] == b',') && b[i + 1].is_ascii_digit() {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        let frac = &s[start..end.min(start + 9)];
        ns = frac.parse::<i64>().ok()? * 10_i64.pow(9 - frac.len() as u32);
        i = end;
    }
    let offset = match b.get(i) {
        Some(b'Z') if i + 1 == b.len() => 0,
        Some(&sign @ (b'+' | b'-')) if b.len() == i + 6 && b[i + 3] == b':' => {
            let (oh, om) = (digits(&b[i + 1..], 2)?, digits(&b[i + 4..], 2)?);
            if oh > 24 || om > 60 {
                return None;
            }
            let o = (oh * 3600 + om * 60) * SECOND;
            if sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    if !(1..=12).contains(&mo) || d < 1 || d > days_in(mo, y) || h >= 24 || mi >= 60 || sec >= 60 {
        return None;
    }
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec;
    Some(secs * SECOND + ns - offset)
}

/// Go's Duration.String for whole seconds: 0s, 45s, 1m5s, 2h0m0s.
pub fn duration(ns: i64) -> String {
    let s = (ns + SECOND / 2).div_euclid(SECOND);
    let (h, m, sec) = (s / 3600, s / 60 % 60, s % 60);
    match (h, m) {
        (0, 0) => format!("{sec}s"),
        (0, _) => format!("{m}m{sec}s"),
        _ => format!("{h}h{m}m{sec}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let t = parse_rfc3339("2026-10-26T12:34:56.789Z").unwrap();
        assert_eq!(rfc3339(t), "2026-10-26T12:34:56Z");
        assert_eq!(rfc3339_ms(t), "2026-10-26T12:34:56.789Z");
        assert_eq!(date(t), "2026-10-26");
        assert_eq!(parse_rfc3339("2026-10-26T14:34:56+02:00").map(rfc3339).unwrap(), "2026-10-26T12:34:56Z");
        assert_eq!(parse_rfc3339("2026-10-26T9:00:00Z").map(rfc3339).unwrap(), "2026-10-26T09:00:00Z");
        assert_eq!(parse_date("2024-02-29").map(rfc3339).unwrap(), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn refuses_what_go_refuses() {
        for s in [
            "2026-10-26",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-10-26T24:00:00Z",
            "2026-10-26T00:00:00",
            "2026-10-26 00:00:00Z",
            "2026-10-26T00:00:00z",
            "x",
        ] {
            assert!(parse_rfc3339(s).is_none(), "{s}");
        }
        for s in ["2023-02-29", "2026-1-01", "20261001", "2026-10-01T00:00:00Z"] {
            assert!(parse_date(s).is_none(), "{s}");
        }
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(65 * SECOND), "1m5s");
        assert_eq!(duration(2 * HOUR), "2h0m0s");
        assert_eq!(duration(1_400_000_000), "1s");
    }
}
