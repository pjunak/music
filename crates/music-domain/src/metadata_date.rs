//! Validated calendar dates retaining year, month or day precision.

pub fn metadata_date_year(value: &str) -> Option<u32> {
    let parts = value.split('-').collect::<Vec<_>>();
    if !matches!(parts.len(), 1..=3)
        || parts[0].len() != 4
        || !parts.iter().all(|s| s.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let year = parts[0]
        .parse::<u32>()
        .ok()
        .filter(|y| (1..=9999).contains(y))?;
    if parts.len() > 1 {
        let month = parts[1]
            .parse::<u32>()
            .ok()
            .filter(|m| (1..=12).contains(m) && parts[1].len() == 2)?;
        if parts.len() == 3 {
            let days = match month {
                2 if year.is_multiple_of(400)
                    || year.is_multiple_of(4) && !year.is_multiple_of(100) =>
                {
                    29
                }
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            parts[2]
                .parse::<u32>()
                .ok()
                .filter(|d| (1..=days).contains(d) && parts[2].len() == 2)?;
        }
    }
    Some(year)
}

#[cfg(test)]
mod tests {
    use super::metadata_date_year;
    #[test]
    fn keeps_precision_and_rejects_impossible_calendar_dates() {
        for date in ["2025", "2025-10", "2025-10-17", "2000-02-29"] {
            assert!(metadata_date_year(date).is_some());
        }
        for date in [
            "",
            "2025-02-29",
            "1900-02-29",
            "2025-04-31",
            "0000",
            "2025junk",
            "2025-1",
            "2025-00",
            " 2025",
            "2025-10-17T00:00:00Z",
        ] {
            assert!(metadata_date_year(date).is_none());
        }
    }
}
