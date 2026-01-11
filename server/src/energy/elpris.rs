//! elprisetjustnu.se API client
//!
//! Fetches raw spot prices from Nord Pool via elprisetjustnu.se.
//! No authentication required - free public API.
//!
//! API format: `GET <https://www.elprisetjustnu.se/api/v1/prices/{YEAR}/{MM}-{DD}_{ZONE}.json>`
//! Returns 96 entries per day (15-min resolution) from October 2025 onward.

use chrono::{DateTime, Datelike, Utc};
use serde::Deserialize;
use tracing::{debug, error, trace, warn};

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
}

impl ElprisClient {
    /// Create a new client for the specified region
    ///
    /// Valid regions: SE1 (Luleå), SE2 (Sundsvall), SE3 (Stockholm), SE4 (Malmö)
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            base_url: ELPRIS_BASE_URL.to_string(),
            region: region.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Create a client with custom base URL (for testing)
    #[cfg(test)]
    pub fn with_base_url(base_url: String, region: String) -> Self {
        Self {
            base_url,
            region,
            client: reqwest::Client::new(),
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
    pub async fn get_today_prices(&self) -> Result<Vec<ElprisEntry>, ElprisError> {
        self.get_prices(Utc::now()).await
    }

    /// Get tomorrow's prices (returns None if not yet available)
    pub async fn get_tomorrow_prices(&self) -> Result<Vec<ElprisEntry>, ElprisError> {
        let tomorrow = Utc::now() + chrono::Duration::days(1);
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

    /// Get the region this client is configured for
    pub fn region(&self) -> &str {
        &self.region
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
    fn test_elpris_client_new() {
        let client = ElprisClient::new("SE3");
        assert_eq!(client.region(), "SE3");
    }

    #[test]
    fn test_elpris_error_display() {
        assert_eq!(
            ElprisError::NotAvailable.to_string(),
            "Prices not available"
        );
        assert_eq!(ElprisError::Http(404).to_string(), "HTTP error: 404");
    }
}
