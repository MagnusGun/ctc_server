//! elprisetjustnu.se API client
//!
//! Fetches raw spot prices from Nord Pool via elprisetjustnu.se.
//! No authentication required - free public API.
//!
//! API format: `GET <https://www.elprisetjustnu.se/api/v1/prices/{YEAR}/{MM}-{DD}_{ZONE}.json>`
//! Returns 96 entries per day (15-min resolution) from October 2025 onward.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use tracing::{debug, error, trace, warn};

use super::tariff::system_time_to_local;

const ELPRIS_BASE_URL: &str = "https://www.elprisetjustnu.se/api/v1/prices";

/// Response entry from elprisetjustnu.se API
#[derive(Debug, Deserialize, Clone)]
pub struct ElprisEntry {
    /// Price in SEK per kWh
    #[serde(rename = "SEK_per_kWh")]
    pub sek_per_kwh: f64,
    /// Price in EUR per kWh
    #[serde(rename = "EUR_per_kWh")]
    pub eur_per_kwh: f64,
    /// EUR to SEK exchange rate
    #[serde(rename = "EXR")]
    pub exchange_rate: f64,
    /// Start of price period (ISO 8601)
    pub time_start: String,
    /// End of price period (ISO 8601)
    pub time_end: String,
}

/// Client for elprisetjustnu.se API
#[derive(Clone)]
pub struct ElprisClient {
    base_url: String,
    region: String,
    client: reqwest::Client,
    /// Timezone used to derive "today"'s date for the URL. Sweden's
    /// electricity zones span a single timezone, but the deployment may be
    /// elsewhere — we want the user's calendar day, not UTC.
    tz: Tz,
}

impl ElprisClient {
    /// Create a new client for the specified region and local timezone.
    ///
    /// Valid regions: SE1 (Luleå), SE2 (Sundsvall), SE3 (Stockholm), SE4 (Malmö)
    pub fn new(region: impl Into<String>, tz: Tz) -> Self {
        Self {
            base_url: ELPRIS_BASE_URL.to_string(),
            region: region.into(),
            client: crate::energy::http_client().clone(),
            tz,
        }
    }

    /// Get prices for a specific date
    pub async fn get_prices(&self, date: DateTime<Utc>) -> Result<Vec<ElprisEntry>, ElprisError> {
        let url = format!(
            "{}/{}/{:02}-{:02}_{}.json",
            self.base_url,
            date.year(),
            date.month(),
            date.day(),
            self.region
        );

        debug!("Fetching prices from: {}", url);

        let response = self
            .client
            .get(&url)
            .header("User-Agent", "CTC-Server/1.0")
            .send()
            .await
            .map_err(|e| ElprisError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 404 {
                // Prices not available yet (common for tomorrow before 13:00)
                trace!("Prices not available for {} (404)", date.format("%Y-%m-%d"));
                return Err(ElprisError::NotAvailable);
            }
            error!("HTTP error {} fetching prices", status);
            return Err(ElprisError::Http(status.as_u16()));
        }

        let prices: Vec<ElprisEntry> = response
            .json()
            .await
            .map_err(|e| ElprisError::Parse(e.to_string()))?;

        debug!(
            "Fetched {} price entries for {}",
            prices.len(),
            date.format("%Y-%m-%d")
        );

        Ok(prices)
    }

    /// Get today's prices
    ///
    /// "Today" is derived from local time in the configured `tz`, not UTC.
    /// Between local midnight and UTC midnight, the UTC date is still
    /// "yesterday" by the user's reckoning, so using `Utc::now()` would
    /// return yesterday's prices in the evening.
    pub async fn get_today_prices(&self) -> Result<Vec<ElprisEntry>, ElprisError> {
        self.get_prices(local_today_as_utc(SystemTime::now(), self.tz))
            .await
    }

    /// Get tomorrow's prices (returns None if not yet available)
    pub async fn get_tomorrow_prices(&self) -> Result<Vec<ElprisEntry>, ElprisError> {
        let tomorrow = local_today_as_utc(SystemTime::now(), self.tz) + chrono::Duration::days(1);
        self.get_prices(tomorrow).await
    }

    /// Try to get tomorrow's prices, returning None if not available
    ///
    /// Tomorrow's prices are typically available after 13:00 CET
    pub async fn try_get_tomorrow_prices(&self) -> Option<Vec<ElprisEntry>> {
        match self.get_tomorrow_prices().await {
            Ok(prices) => {
                if prices.is_empty() {
                    warn!("Tomorrow's prices returned empty array");
                    None
                } else {
                    Some(prices)
                }
            }
            Err(ElprisError::NotAvailable) => {
                debug!("Tomorrow's prices not yet available");
                None
            }
            Err(e) => {
                warn!("Failed to fetch tomorrow's prices: {}", e);
                None
            }
        }
    }

}

/// Error types for elpris API
#[derive(Debug)]
pub enum ElprisError {
    /// Network/connection error
    Network(String),
    /// HTTP error response
    Http(u16),
    /// JSON parsing error
    Parse(String),
    /// Prices not available (404)
    NotAvailable,
}

impl std::fmt::Display for ElprisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "Network error: {e}"),
            Self::Http(code) => write!(f, "HTTP error: {code}"),
            Self::Parse(e) => write!(f, "Parse error: {e}"),
            Self::NotAvailable => write!(f, "Prices not available"),
        }
    }
}

impl std::error::Error for ElprisError {}

/// Return a `DateTime<Utc>` whose calendar date matches today in `tz`-local
/// time.
///
/// `ElprisClient::get_prices` only consumes year/month/day from the value, so
/// we embed the local date into a UTC noon timestamp. Using noon avoids any
/// risk that further timezone math elsewhere shifts the date.
fn local_today_as_utc(now: SystemTime, tz: Tz) -> DateTime<Utc> {
    let (year, month, day, _hour) = system_time_to_local(now, tz);
    Utc.with_ymd_and_hms(year, month, day, 12, 0, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elpris_entry_deserialize() {
        let json = r#"{
            "SEK_per_kWh": 0.75,
            "EUR_per_kWh": 0.065,
            "EXR": 11.54,
            "time_start": "2026-01-04T14:00:00+01:00",
            "time_end": "2026-01-04T14:15:00+01:00"
        }"#;

        let entry: ElprisEntry = serde_json::from_str(json).unwrap();
        assert!((entry.sek_per_kwh - 0.75).abs() < f64::EPSILON);
        assert!((entry.eur_per_kwh - 0.065).abs() < f64::EPSILON);
        assert!((entry.exchange_rate - 11.54).abs() < f64::EPSILON);
        assert_eq!(entry.time_start, "2026-01-04T14:00:00+01:00");
        assert_eq!(entry.time_end, "2026-01-04T14:15:00+01:00");
    }

    #[test]
    fn test_elpris_error_display() {
        assert_eq!(
            ElprisError::NotAvailable.to_string(),
            "Prices not available"
        );
        assert_eq!(ElprisError::Http(404).to_string(), "HTTP error: 404");
    }

    #[test]
    #[allow(clippy::cast_sign_loss)]
    fn test_swedish_today_uses_local_date() {
        use std::time::Duration;

        // 2026-01-15 23:30 UTC — Swedish local time is 00:30 on 2026-01-16,
        // so "today" should be the 16th, not the 15th.
        // 2026-01-15 23:30 UTC = days_since_epoch(2026-01-15) * 86400 + 23*3600 + 30*60
        // Use a known timestamp: 2026-01-15T23:30:00Z
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 1, 15, 23, 30, 0)
            .unwrap()
            .timestamp() as u64;
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);

        let today = local_today_as_utc(now, chrono_tz::Europe::Stockholm);
        assert_eq!(today.year(), 2026);
        assert_eq!(today.month(), 1);
        assert_eq!(today.day(), 16);
    }

    #[test]
    #[allow(clippy::cast_sign_loss)]
    fn test_swedish_today_midday_utc_matches() {
        use std::time::Duration;

        // 2026-06-15 12:00 UTC — Swedish local time is 13:00, same date.
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 6, 15, 12, 0, 0)
            .unwrap()
            .timestamp() as u64;
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);

        let today = local_today_as_utc(now, chrono_tz::Europe::Stockholm);
        assert_eq!(today.year(), 2026);
        assert_eq!(today.month(), 6);
        assert_eq!(today.day(), 15);
    }
}
