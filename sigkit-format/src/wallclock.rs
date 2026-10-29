//! UTC formatting without a date crate: enough for `created` and anchor readouts.

/// `unix_s` seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339_utc(unix_s: i64) -> String {
    let days = unix_s.div_euclid(86_400);
    let of_day = unix_s.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        (of_day % 3600) / 60,
        of_day % 60
    )
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    rfc3339_utc(secs)
}

#[cfg(test)]
mod tests {
    use super::rfc3339_utc;

    #[test]
    fn formats_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_761_739_200), "2025-10-29T12:00:00Z");
        assert_eq!(rfc3339_utc(1_793_275_200), "2026-10-29T12:00:00Z");
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59Z");
    }
}
