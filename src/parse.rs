use chrono::{Duration, NaiveDate, NaiveTime};

/// Parses "H:MM" or "HH:MM" into a NaiveTime with a clear error message.
pub fn parse_time(s: &str) -> Result<NaiveTime, String> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid time '{s}', expected H:MM or HH:MM"))?;
    let hour: u32 = h
        .parse()
        .map_err(|_| format!("invalid time '{s}', expected H:MM or HH:MM"))?;
    let minute: u32 = m
        .parse()
        .map_err(|_| format!("invalid time '{s}', expected H:MM or HH:MM"))?;
    NaiveTime::from_hms_opt(hour, minute, 0)
        .ok_or_else(|| format!("invalid time '{s}', hour/minute out of range"))
}

/// Parses "H:MM-H:MM" into a (start, end) pair, requiring start < end.
pub fn parse_range(s: &str) -> Result<(NaiveTime, NaiveTime), String> {
    let (start_s, end_s) = s
        .split_once('-')
        .ok_or_else(|| format!("invalid range '{s}', expected START-END e.g. 8:00-12:00"))?;
    let start = parse_time(start_s.trim())?;
    let end = parse_time(end_s.trim())?;
    if start >= end {
        return Err(format!("invalid range '{s}': start must be before end"));
    }
    Ok((start, end))
}

/// Resolves a day-spec ("today"/"yesterday") relative to `today`.
pub fn resolve_day_spec(spec: &str, today: NaiveDate) -> Result<NaiveDate, String> {
    match spec.to_lowercase().as_str() {
        "today" => Ok(today),
        "yesterday" => Ok(today - Duration::days(1)),
        other => Err(format!(
            "invalid day '{other}', expected 'today' or 'yesterday'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_range() {
        let (s, e) = parse_range("8:00-12:00").unwrap();
        assert_eq!(s, NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        assert_eq!(e, NaiveTime::from_hms_opt(12, 0, 0).unwrap());
    }

    #[test]
    fn rejects_inverted_range() {
        assert!(parse_range("12:00-8:00").is_err());
    }

    #[test]
    fn rejects_equal_range() {
        assert!(parse_range("8:00-8:00").is_err());
    }

    #[test]
    fn rejects_malformed_time() {
        assert!(parse_range("8-12:00").is_err());
    }

    #[test]
    fn resolves_day_spec() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 11).unwrap();
        assert_eq!(resolve_day_spec("today", today).unwrap(), today);
        assert_eq!(
            resolve_day_spec("yesterday", today).unwrap(),
            NaiveDate::from_ymd_opt(2026, 9, 10).unwrap()
        );
        assert!(resolve_day_spec("tomorrow", today).is_err());
    }
}
