use std::time::{SystemTime, UNIX_EPOCH};

/// Render a file modification time as a compact local date, e.g. 2026-08-12 14:30.
pub fn format_time(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => format_timestamp(duration.as_secs() as i64),
        Err(_) => "before 1970".into(),
    }
}

fn format_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_epoch_as_date() {
        let t = UNIX_EPOCH + Duration::from_secs(0);
        assert_eq!(format_time(t), "1970-01-01 00:00");
    }

    #[test]
    fn formats_known_timestamp() {
        // 2026-08-12 14:30 UTC -> verify civil date math stays stable.
        let t = UNIX_EPOCH + Duration::from_secs(1786525800);
        let out = format_time(t);
        assert!(out.starts_with("2026-08-12"), "got {out}");
    }
}
