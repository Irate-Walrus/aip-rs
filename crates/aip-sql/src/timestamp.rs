//! RFC3339 timestamp rendering — the canonical text the transpiler binds.

/// Format a protobuf [`Timestamp`](prost_types::Timestamp) as a canonical
/// RFC3339 UTC string at second precision.
///
/// This is **exactly** the text the `aip-sql` transpiler binds when a filter
/// contains a `timestamp(...)` literal. Storing columns with this formatter
/// guarantees lexicographic ordering matches chronological ordering and that
/// stored values compare correctly with bound filter literals — by
/// construction, not convention.
///
/// Only the non-negative range is handled (negative seconds clamp to zero).
/// Nanoseconds are truncated; only whole-second precision is represented.
pub fn format_timestamp(ts: &prost_types::Timestamp) -> String {
    let secs = ts.seconds.max(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, minute, second) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::Timestamp;

    #[test]
    fn unix_epoch() {
        assert_eq!(
            format_timestamp(&Timestamp { seconds: 0, nanos: 0 }),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn known_date() {
        // 2024-03-15T11:34:56Z = 1710502496 Unix seconds
        assert_eq!(
            format_timestamp(&Timestamp { seconds: 1710502496, nanos: 0 }),
            "2024-03-15T11:34:56Z"
        );
    }

    #[test]
    fn negative_clamps_to_epoch() {
        assert_eq!(
            format_timestamp(&Timestamp { seconds: -1, nanos: 0 }),
            "1970-01-01T00:00:00Z"
        );
    }

    #[test]
    fn nanoseconds_truncated() {
        assert_eq!(
            format_timestamp(&Timestamp { seconds: 1710502496, nanos: 999_999_999 }),
            "2024-03-15T11:34:56Z"
        );
    }
}
