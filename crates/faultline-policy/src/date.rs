//! Calendar dates for waiver expiry, without a date library.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A date in the proleptic Gregorian calendar (UTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    pub fn new(year: i32, month: u8, day: u8) -> Option<Date> {
        (1..=12)
            .contains(&month)
            .then_some(())
            .filter(|()| day >= 1 && day <= days_in_month(year, month))
            .map(|()| Date { year, month, day })
    }

    /// Parses `YYYY-MM-DD`.
    pub fn parse(text: &str) -> Option<Date> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            text.get(range)?
                .chars()
                .try_fold(0u32, |acc, c| Some(acc * 10 + c.to_digit(10)?))
        };
        Date::new(
            i32::try_from(digits(0..4)?).ok()?,
            u8::try_from(digits(5..7)?).ok()?,
            u8::try_from(digits(8..10)?).ok()?,
        )
    }

    /// Today's date in UTC.
    pub fn today() -> Date {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Date::from_days_since_epoch(i64::try_from(seconds / 86_400).unwrap_or(0))
    }

    /// The civil date `days` after 1970-01-01 (Howard Hinnant's
    /// `civil_from_days` algorithm).
    pub fn from_days_since_epoch(days: i64) -> Date {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        Date {
            year: i32::try_from(year).unwrap_or(i32::MAX),
            month: u8::try_from(month).unwrap_or(1),
            day: u8::try_from(day).unwrap_or(1),
        }
    }
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::Date;

    #[test]
    fn parsing_validates_the_calendar() {
        assert_eq!(Date::parse("2024-02-29").unwrap().to_string(), "2024-02-29");
        for bad in [
            "2023-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-4-01",
            "2026/04/01",
            "20260401",
            "2026-04-31",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad}");
        }
        assert!(Date::parse("2026-01-31").unwrap() < Date::parse("2026-02-01").unwrap());
    }

    #[test]
    fn epoch_days_convert_to_civil_dates() {
        assert_eq!(Date::from_days_since_epoch(0).to_string(), "1970-01-01");
        assert_eq!(
            Date::from_days_since_epoch(11_016).to_string(),
            "2000-02-29"
        );
        assert_eq!(
            Date::from_days_since_epoch(20_724).to_string(),
            "2026-09-28"
        );
        assert_eq!(Date::from_days_since_epoch(-1).to_string(), "1969-12-31");
    }
}
