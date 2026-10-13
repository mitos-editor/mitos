use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

/// Format elapsed UTC seconds; offsets do not affect relative time.
pub(super) fn format_relative_time(timestamp: i64, now: i64) -> String {
    let elapsed = i128::from(now) - i128::from(timestamp);
    let seconds = elapsed.unsigned_abs();
    let (duration, unit) = [
        (365 * 24 * 60 * 60, "year"),
        (30 * 24 * 60 * 60, "month"),
        (24 * 60 * 60, "day"),
        (60 * 60, "hour"),
        (60, "minute"),
        (1, "second"),
    ]
    .into_iter()
    .find(|(duration, _)| seconds >= *duration)
    .unwrap_or((1, "second"));
    let value = seconds / duration;
    let plural = if value == 1 { "" } else { "s" };
    let direction = if elapsed >= 0 { "ago" } else { "from now" };
    format!("{value} {unit}{plural} {direction}")
}

#[cfg(test)]
mod tests {
    use super::format_relative_time;

    #[test]
    fn relative_time_handles_unit_boundaries_and_future_dates() {
        for (seconds, expected) in [
            (0, "0 seconds ago"),
            (1, "1 second ago"),
            (59, "59 seconds ago"),
            (60, "1 minute ago"),
            (120, "2 minutes ago"),
            (3599, "59 minutes ago"),
            (3600, "1 hour ago"),
            (86399, "23 hours ago"),
            (86400, "1 day ago"),
            (30 * 86400, "1 month ago"),
            (365 * 86400, "1 year ago"),
            (-1, "1 second from now"),
            (-120, "2 minutes from now"),
        ] {
            assert_eq!(
                format_relative_time(1_000_000, 1_000_000 + seconds),
                expected
            );
        }
    }

    #[test]
    fn extreme_timestamps_do_not_overflow() {
        assert!(format_relative_time(i64::MIN, i64::MAX).ends_with("years ago"));
        assert!(format_relative_time(i64::MAX, i64::MIN).ends_with("years from now"));
    }
}
