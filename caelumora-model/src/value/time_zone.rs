//! The time zone a session is held in (issue 424): `TimeZone`, an IANA name
//! such as `Europe/Berlin`. The times a model is given of the messages of
//! others are written in it, in one form for every language.

use super::ValueError;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// What a time zone looks like, in the error for one that is none.
const EXPECTED: &str = "an IANA time zone such as \"Europe/Berlin\" or \"UTC\"";

/// A time zone by its IANA name, with its rules - offsets and daylight
/// saving time - as compiled in, so that a time is written the same on
/// every machine. Only names the database knows are time zones, written
/// exactly as it writes them (`Europe/Berlin`, not `europe/berlin`).
/// Written down as its name, and checked when read.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeZone {
    zone: Tz,
}

impl TimeZone {
    /// Coordinated universal time: where nobody chose another.
    pub const UTC: Self = Self { zone: Tz::UTC };

    /// The time zone called `name`.
    ///
    /// # Errors
    /// `ValueError` if it is empty or no name the time zone database knows.
    pub fn new(name: impl AsRef<str>) -> Result<Self, ValueError> {
        let name = name.as_ref();
        if name.is_empty() {
            return Err(ValueError::Empty { field: Self::FIELD });
        }
        let zone = name.parse::<Tz>().map_err(|_| ValueError::Malformed { field: Self::FIELD, expected: EXPECTED })?;
        Ok(Self { zone })
    }

    /// Its IANA name.
    pub fn as_str(&self) -> &str {
        self.zone.name()
    }

    /// `at` as a model is given it, in this time zone: `Sun 18 Oct 2026,
    /// 14:32` - the weekday by name, which models use and do not work out
    /// from a date, in English names for every language, which models read
    /// in a conversation of any (`work/message_times/RESULTS.md`, findings
    /// 1 and 7), and without the zone, which the session names once.
    pub fn format(&self, at: DateTime<Utc>) -> String {
        at.with_timezone(&self.zone).format("%a %-d %b %Y, %H:%M").to_string()
    }

    /// `at` written out, in this time zone: `Sunday, 18 October 2026,
    /// 14:33` - as a model is told the current time, once, where length
    /// costs little.
    pub fn format_long(&self, at: DateTime<Utc>) -> String {
        at.with_timezone(&self.zone).format("%A, %-d %B %Y, %H:%M").to_string()
    }
}

impl_value!([] TimeZone, "time zone");

/// UTC, as for a session that names none.
impl Default for TimeZone {
    fn default() -> Self {
        Self::UTC
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn only_a_name_the_database_knows_is_a_time_zone() {
        assert_eq!(TimeZone::new("Europe/Berlin").unwrap().as_str(), "Europe/Berlin");
        assert_eq!(TimeZone::new("UTC").unwrap(), TimeZone::UTC);
        assert!(matches!(TimeZone::new(""), Err(ValueError::Empty { .. })));
        assert!(matches!(TimeZone::new("Europe/Atlantis"), Err(ValueError::Malformed { .. })), "no such zone");
        assert!(TimeZone::new("europe/berlin").is_err(), "written as the database writes it");
        assert!(TimeZone::new(" Europe/Berlin").is_err(), "nothing is trimmed");
    }

    #[test]
    fn a_time_is_written_in_the_zone_with_its_weekday() {
        let at = Utc.with_ymd_and_hms(2026, 10, 18, 12, 32, 0).unwrap();
        assert_eq!(TimeZone::UTC.format(at), "Sun 18 Oct 2026, 12:32");
        assert_eq!(TimeZone::new("Europe/Berlin").unwrap().format(at), "Sun 18 Oct 2026, 14:32", "summer time");
        let winter = Utc.with_ymd_and_hms(2026, 12, 1, 8, 5, 0).unwrap();
        assert_eq!(TimeZone::new("Europe/Berlin").unwrap().format(winter), "Tue 1 Dec 2026, 09:05", "winter time, the day without a leading zero");
        assert_eq!(TimeZone::new("America/New_York").unwrap().format(at), "Sun 18 Oct 2026, 08:32");
        assert_eq!(TimeZone::new("Europe/Berlin").unwrap().format_long(winter), "Tuesday, 1 December 2026, 09:05", "written out");
    }

    #[test]
    fn a_time_zone_is_written_as_its_name_and_checked_when_read() {
        let zone = TimeZone::new("Asia/Tokyo").unwrap();
        assert_eq!(serde_json::to_value(zone).unwrap(), serde_json::json!("Asia/Tokyo"));
        assert_eq!(serde_json::from_value::<TimeZone>(serde_json::json!("Asia/Tokyo")).unwrap(), zone);
        assert!(serde_json::from_value::<TimeZone>(serde_json::json!("Mars/Olympus")).is_err());
    }
}
