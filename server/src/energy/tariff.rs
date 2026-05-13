//! Tariff calculation for Göteborg Energi time-of-use pricing
//!
//! Tariff schedule:
//! - Winter (Nov 1 - Mar 31): High tariff on weekdays 07:00-20:00, low otherwise
//! - Summer (Apr 1 - Oct 31): Low tariff 24/7
//!
//! Uses Swedish local time (CET in winter, CEST in summer). DST follows the
//! `Europe/Stockholm` IANA zone via `chrono_tz`. Swedish holidays
//! ("röda dagar") are treated as weekends.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Europe::Stockholm;
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

/// Get tariff mode for a specific time. The Göteborg Energi tariff calendar
/// is Swedish-specific, so the timezone is hardcoded to `Europe/Stockholm`
/// regardless of the deployment's local time.
#[must_use]
pub fn get_tariff_at(time: SystemTime) -> TariffMode {
    let (year, month, day, hour) = system_time_to_local(time, Stockholm);
    let weekday = day_of_week(year, month, day);

    is_high_tariff(year, month, day, hour, weekday)
}

/// Convert `SystemTime` to local time components in the given timezone,
/// accounting for DST.
pub(crate) fn system_time_to_local(time: SystemTime, tz: chrono_tz::Tz) -> (i32, u32, u32, u32) {
    let utc: DateTime<Utc> = time.into();
    let local = utc.with_timezone(&tz);
    (local.year(), local.month(), local.day(), local.hour())
}

/// Return the UTC Unix-seconds of local midnight on the given date in `tz`.
/// On the ambiguous fall-back day (rare — most IANA zones jump at 02:00 or
/// 03:00, never midnight) the earliest mapping is chosen.
pub(crate) fn local_midnight_utc_secs(year: i32, month: u32, day: u32, tz: chrono_tz::Tz) -> u64 {
    let naive = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("valid local date");
    let local = tz
        .from_local_datetime(&naive)
        .earliest()
        .expect("local midnight is unambiguous in practice");
    #[allow(clippy::cast_sign_loss)]
    {
        local.timestamp() as u64
    }
}

/// Day-of-month of the last Sunday in the given month.
///
/// Only used by test helpers now that DST handling moved to `chrono_tz`; kept
/// behind `#[cfg(test)]` so it doesn't trip the dead-code lint.
#[cfg(test)]
fn last_sunday_of_month(year: i32, month: u32) -> u32 {
    // Months Mar and Oct always have 31 days.
    let last_day = 31u32;
    let weekday = day_of_week(year, month, last_day);
    // weekday 0 = Sunday. Subtract weekday days to reach the most recent Sunday.
    last_day - weekday
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
        // Convert Swedish local time to UTC. Use a coarse DST check on the
        // local date (good enough for test inputs that never sit on the
        // ambiguous transition hour itself).
        let local_is_cest = match month {
            4..=9 => true,
            3 => day >= last_sunday_of_month(year, 3),
            10 => day < last_sunday_of_month(year, 10),
            _ => false,
        };
        let offset_secs: i64 = if local_is_cest { 7200 } else { 3600 };
        let days = ymd_to_days(year, month, day);
        #[allow(clippy::cast_sign_loss)]
        // days is always positive for dates after 1970
        let secs = (days * 86400 + i64::from(hour) * 3600 - offset_secs) as u64;
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

    /// Parametric sweep across a 20-year window. Reference dates from the
    /// Swedish Almanacka / Catholic Easter tables. Guards against subtle
    /// drift in the Anonymous Gregorian implementation that single-year
    /// tests would miss.
    #[test]
    fn test_easter_parametric_sweep() {
        let cases: &[(i32, u32, u32)] = &[
            (2010, 4, 4),
            (2011, 4, 24),
            (2012, 4, 8),
            (2013, 3, 31),
            (2014, 4, 20),
            (2015, 4, 5),
            (2016, 3, 27),
            (2017, 4, 16),
            (2018, 4, 1),
            (2019, 4, 21),
            (2020, 4, 12),
            (2021, 4, 4),
            (2022, 4, 17),
            (2023, 4, 9),
            (2024, 3, 31),
            (2025, 4, 20),
            (2026, 4, 5),
            (2027, 3, 28),
            (2028, 4, 16),
            (2029, 4, 1),
            (2030, 4, 21),
        ];
        for &(year, month, day) in cases {
            assert_eq!(
                calculate_easter(year),
                (month, day),
                "Easter Sunday for {year} should be {month:02}-{day:02}"
            );
        }
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

    #[test]
    fn test_last_sunday_of_month() {
        // DST 2025: spring forward Mar 30, fall back Oct 26.
        assert_eq!(last_sunday_of_month(2025, 3), 30);
        assert_eq!(last_sunday_of_month(2025, 10), 26);
        // DST 2026: spring forward Mar 29, fall back Oct 25.
        assert_eq!(last_sunday_of_month(2026, 3), 29);
        assert_eq!(last_sunday_of_month(2026, 10), 25);
    }

    #[test]
    fn test_summer_dst_conversion() {
        // 2026-07-15 12:00 UTC should map to 14:00 Swedish local (CEST).
        let days = ymd_to_days(2026, 7, 15);
        #[allow(clippy::cast_sign_loss)]
        let secs = (days * 86400 + 12 * 3600) as u64;
        let utc = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
        let (y, mo, d, h) = system_time_to_local(utc, Stockholm);
        assert_eq!((y, mo, d, h), (2026, 7, 15, 14));
    }

    #[test]
    fn test_spring_dst_transition() {
        // 2026 last Sunday of March is the 29th. At 00:00 UTC it's still CET
        // (Swedish local 01:00). At 01:00 UTC the clock jumps to 03:00 CEST.
        let days = ymd_to_days(2026, 3, 29);

        #[allow(clippy::cast_sign_loss)]
        let utc_before = SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400) as u64);
        let (_, _, _, h_before) = system_time_to_local(utc_before, Stockholm);
        assert_eq!(h_before, 1); // 00:00 UTC -> 01:00 CET

        #[allow(clippy::cast_sign_loss)]
        let utc_after =
            SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400 + 2 * 3600) as u64);
        let (y, mo, d, h_after) = system_time_to_local(utc_after, Stockholm);
        assert_eq!((y, mo, d, h_after), (2026, 3, 29, 4)); // 02:00 UTC -> 04:00 CEST
    }

    #[test]
    fn test_fall_dst_transition() {
        // 2025 last Sunday of October is the 26th. At 00:00 UTC it's still
        // CEST (Swedish local 02:00). At 01:00 UTC the clock falls back to
        // 02:00 CET.
        let days = ymd_to_days(2025, 10, 26);

        #[allow(clippy::cast_sign_loss)]
        let utc_before = SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400) as u64);
        let (_, _, _, h_before) = system_time_to_local(utc_before, Stockholm);
        assert_eq!(h_before, 2); // 00:00 UTC -> 02:00 CEST

        #[allow(clippy::cast_sign_loss)]
        let utc_after = SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400 + 3600) as u64);
        let (_, _, _, h_after) = system_time_to_local(utc_after, Stockholm);
        assert_eq!(h_after, 2); // 01:00 UTC -> 02:00 CET
    }

    /// During fall-back Sunday, two distinct UTC instants both map to local
    /// 02:30. Each instant is unambiguous on the Unix timeline, but the
    /// presented local time is ambiguous. The tariff lookup must still pick
    /// `Low` consistently for both (Sunday is always low, regardless of hour),
    /// so a peak straddling the rollback is not double-counted under "high"
    /// for the second pass through the 02:00 hour.
    #[test]
    fn test_tariff_at_fall_back_ambiguous_hour_is_consistent() {
        let days = ymd_to_days(2025, 10, 26);
        // 00:30 UTC (= 02:30 CEST, pre fall-back)
        #[allow(clippy::cast_sign_loss)]
        let pre = SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400 + 1800) as u64);
        // 01:30 UTC (= 02:30 CET, post fall-back)
        #[allow(clippy::cast_sign_loss)]
        let post =
            SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400 + 3600 + 1800) as u64);
        assert_eq!(get_tariff_at(pre), TariffMode::Low);
        assert_eq!(get_tariff_at(post), TariffMode::Low);
    }

    /// Non-Stockholm timezone: the same UTC instant maps to a different local
    /// (year, month, day, hour) tuple when interpreted in `America/New_York`.
    /// On 2026-07-15 the zone is in EDT (UTC-4).
    #[test]
    fn system_time_to_local_handles_non_stockholm_tz() {
        let days = ymd_to_days(2026, 7, 15);
        #[allow(clippy::cast_sign_loss)]
        let utc = SystemTime::UNIX_EPOCH + Duration::from_secs((days * 86400 + 12 * 3600) as u64);
        let (y, mo, d, h) = system_time_to_local(utc, chrono_tz::America::New_York);
        assert_eq!((y, mo, d, h), (2026, 7, 15, 8));
    }
}
