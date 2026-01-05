//! Tariff calculation for Göteborg Energi time-of-use pricing
//!
//! Tariff schedule:
//! - Winter (Nov 1 - Mar 31): High tariff on weekdays 07:00-20:00, low otherwise
//! - Summer (Apr 1 - Oct 31): Low tariff 24/7
//!
//! Uses Swedish standard time (UTC+1) year-round, ignoring daylight saving time.
//! Swedish holidays ("röda dagar") are treated as weekends.

use std::time::SystemTime;

use serde::Serialize;

/// Tariff mode based on time-of-use pricing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TariffMode {
    /// High tariff (högtariff) - expensive
    High,
    /// Low tariff (lågtariff) - cheap
    Low,
}

impl std::fmt::Display for TariffMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::High => write!(f, "high"),
            Self::Low => write!(f, "low"),
        }
    }
}

/// Get the current tariff mode based on Göteborg Energi schedule
#[must_use]
pub fn get_current_tariff() -> TariffMode {
    let now = SystemTime::now();
    get_tariff_at(now)
}

/// Get tariff mode for a specific time
#[must_use]
pub fn get_tariff_at(time: SystemTime) -> TariffMode {
    let (year, month, day, hour) = system_time_to_swedish(time);
    let weekday = day_of_week(year, month, day);

    is_high_tariff(year, month, day, hour, weekday)
}

/// Convert `SystemTime` to Swedish standard time components (UTC+1)
fn system_time_to_swedish(time: SystemTime) -> (i32, u32, u32, u32) {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    // Add 1 hour for Swedish standard time (UTC+1)
    let secs = duration.as_secs() + 3600;

    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hour = (time_of_day / 3600) as u32;

    let (year, month, day) = days_to_ymd(days);
    (year, month, day, hour)
}

/// Determine if high tariff applies based on date/time
fn is_high_tariff(year: i32, month: u32, day: u32, hour: u32, weekday: u32) -> TariffMode {
    // Summer (Apr 1 - Oct 31): Always low tariff
    if (4..=10).contains(&month) {
        return TariffMode::Low;
    }

    // Winter (Nov 1 - Mar 31): Check time and day
    // Weekend (Saturday=6, Sunday=0): Low tariff
    if weekday == 0 || weekday == 6 {
        return TariffMode::Low;
    }

    // Swedish holiday: Low tariff
    if is_swedish_holiday(year, month, day) {
        return TariffMode::Low;
    }

    // Weekday, check hours (07:00-19:59 = high, 20:00-06:59 = low)
    if (7..20).contains(&hour) {
        TariffMode::High
    } else {
        TariffMode::Low
    }
}

/// Check if date is a Swedish "red day" per Göteborg Energi's tariff schedule.
///
/// NOTE: This only includes holidays explicitly listed by Göteborg Energi for
/// their time-of-use tariff. Other Swedish public holidays (Första maj,
/// Nationaldagen, Kristi himmelsfärd, Midsommar, Alla helgons dag, etc.)
/// are NOT included because:
/// - Summer holidays (Apr 1 - Oct 31) are already covered by low tariff period
/// - Weekend holidays (Midsommardagen, Alla helgons dag) are already Saturdays
///
/// Reference: <https://www.goteborgenergi.se/privat/elnat/nya-elnatsavgiftsmodellen>
fn is_swedish_holiday(year: i32, month: u32, day: u32) -> bool {
    // Fixed holidays per Göteborg Energi specification
    #[allow(clippy::match_same_arms)]
    // Comments explain each holiday, keeping arms separate for documentation
    match (month, day) {
        (1, 1) => return true,   // Nyårsdagen
        (1, 6) => return true,   // Trettondag jul
        (12, 24) => return true, // Julafton
        (12, 25) => return true, // Juldagen
        (12, 26) => return true, // Annandag jul
        (12, 31) => return true, // Nyårsafton
        _ => {}
    }

    // Easter-based holidays (movable) per Göteborg Energi specification
    let easter = calculate_easter(year);
    let easter_days = ymd_to_days(year, easter.0, easter.1);
    let current_days = ymd_to_days(year, month, day);

    // Långfredagen (Good Friday) = Easter - 2
    if current_days == easter_days - 2 {
        return true;
    }
    // Annandag påsk (Easter Monday) = Easter + 1
    if current_days == easter_days + 1 {
        return true;
    }

    false
}

/// Calculate Easter Sunday using the Anonymous Gregorian algorithm
#[allow(clippy::many_single_char_names)]
// Variable names a-m follow the standard Anonymous Gregorian algorithm notation
fn calculate_easter(year: i32) -> (u32, u32) {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = ((h + l - 7 * m + 114) % 31) + 1;

    #[allow(clippy::cast_sign_loss)]
    (month as u32, day as u32)
}

/// Convert days since Unix epoch to (year, month, day)
#[allow(clippy::similar_names)]
// doe/doy are standard names in Howard Hinnant's date algorithm
fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let year = if m <= 2 { y + 1 } else { y } as i32;

    #[allow(clippy::cast_possible_truncation)]
    (year, m as u32, d as u32)
}

/// Convert (year, month, day) to days since Unix epoch
#[allow(clippy::similar_names)]
// doy/day are different: doy = day of year, day = day of month
fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let m = i64::from(if month <= 2 { month + 12 } else { month });

    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_468
}

/// Calculate day of week (0=Sunday, 1=Monday, ..., 6=Saturday)
fn day_of_week(year: i32, month: u32, day: u32) -> u32 {
    let days = ymd_to_days(year, month, day);
    // Jan 1, 1970 was a Thursday (4)
    #[allow(clippy::cast_sign_loss)]
    let weekday = ((days % 7 + 4 + 7) % 7) as u32;
    weekday
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_time(year: i32, month: u32, day: u32, hour: u32) -> SystemTime {
        // Convert to UTC (subtract 1 hour from Swedish time)
        let days = ymd_to_days(year, month, day);
        #[allow(clippy::cast_sign_loss)]
        // days is always positive for dates after 1970
        let secs = (days * 86400 + i64::from(hour) * 3600 - 3600) as u64;
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn test_summer_always_low() {
        // July 15, 2025 at 10:00 (Swedish time) - summer, should be low
        let time = make_time(2025, 7, 15, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // April 1, 2025 at 08:00 - summer starts, should be low
        let time = make_time(2025, 4, 1, 8);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // October 31, 2025 at 19:00 - last day of summer, should be low
        let time = make_time(2025, 10, 31, 19);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_winter_weekday_high_hours() {
        // January 15, 2025 is a Wednesday
        // 08:00 Swedish time - high tariff
        let time = make_time(2025, 1, 15, 8);
        assert_eq!(get_tariff_at(time), TariffMode::High);

        // 19:00 Swedish time - still high tariff
        let time = make_time(2025, 1, 15, 19);
        assert_eq!(get_tariff_at(time), TariffMode::High);
    }

    #[test]
    fn test_winter_weekday_low_hours() {
        // January 15, 2025 is a Wednesday
        // 06:00 Swedish time - low tariff
        let time = make_time(2025, 1, 15, 6);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // 20:00 Swedish time - low tariff
        let time = make_time(2025, 1, 15, 20);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // 23:00 Swedish time - low tariff
        let time = make_time(2025, 1, 15, 23);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_winter_weekend_low() {
        // January 18, 2025 is a Saturday
        let time = make_time(2025, 1, 18, 12);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // January 19, 2025 is a Sunday
        let time = make_time(2025, 1, 19, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_fixed_holidays() {
        // December 25, 2025 is Christmas - should be low even at peak hours
        let time = make_time(2025, 12, 25, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // January 1, 2026 is New Year's Day
        let time = make_time(2026, 1, 1, 12);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // January 6, 2025 is Epiphany
        let time = make_time(2025, 1, 6, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_easter_2025() {
        // Easter 2025 is April 20
        // Good Friday is April 18 - but April is summer, so low anyway
        let easter = calculate_easter(2025);
        assert_eq!(easter, (4, 20));
    }

    #[test]
    fn test_easter_2024() {
        // Easter 2024 is March 31
        let easter = calculate_easter(2024);
        assert_eq!(easter, (3, 31));

        // Good Friday March 29, 2024 - should be low (holiday in winter)
        let time = make_time(2024, 3, 29, 12);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_easter_monday_winter() {
        // Easter 2016 is March 27 → Easter Monday March 28 (still winter)
        let easter = calculate_easter(2016);
        assert_eq!(easter, (3, 27));

        // Easter Monday March 28, 2016 - should be low (röd dag in winter)
        // Monday at 10:00 would normally be high tariff, but it's a holiday
        let time = make_time(2016, 3, 28, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }

    #[test]
    fn test_day_of_week() {
        // January 1, 1970 was Thursday (4)
        assert_eq!(day_of_week(1970, 1, 1), 4);

        // January 15, 2025 is Wednesday (3)
        assert_eq!(day_of_week(2025, 1, 15), 3);

        // January 18, 2025 is Saturday (6)
        assert_eq!(day_of_week(2025, 1, 18), 6);

        // January 19, 2025 is Sunday (0)
        assert_eq!(day_of_week(2025, 1, 19), 0);
    }

    #[test]
    fn test_tariff_mode_display() {
        assert_eq!(TariffMode::High.to_string(), "high");
        assert_eq!(TariffMode::Low.to_string(), "low");
    }

    #[test]
    fn test_winter_boundary_november_1() {
        // November 1, 2025 is Saturday - low (weekend)
        let time = make_time(2025, 11, 1, 10);
        assert_eq!(get_tariff_at(time), TariffMode::Low);

        // November 3, 2025 is Monday - high at 10:00
        let time = make_time(2025, 11, 3, 10);
        assert_eq!(get_tariff_at(time), TariffMode::High);
    }

    #[test]
    fn test_winter_boundary_march_31() {
        // March 31, 2025 is Monday - high at 10:00 (still winter)
        let time = make_time(2025, 3, 31, 10);
        assert_eq!(get_tariff_at(time), TariffMode::High);

        // March 31, 2025 at 06:00 - low (outside peak hours)
        let time = make_time(2025, 3, 31, 6);
        assert_eq!(get_tariff_at(time), TariffMode::Low);
    }
}
