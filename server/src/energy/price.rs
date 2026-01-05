//! Electricity price state management
//!
//! Manages price data from dual sources:
//! - elprisetjustnu.se: Raw spot prices (Nord Pool)
//! - Tibber: Total prices with markup and price levels

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;

/// Thread-safe price state wrapper
#[derive(Clone)]
pub struct PriceState {
    inner: Arc<Mutex<PriceStateInner>>,
}

/// Internal price state
struct PriceStateInner {
    /// Current price point
    current: Option<PricePoint>,
    /// Today's prices (up to 96 entries for 15-min resolution)
    today: Vec<PricePoint>,
    /// Tomorrow's prices (up to 96 entries, available after ~13:00 CET)
    tomorrow: Vec<PricePoint>,
    /// Last update timestamp
    last_updated: Option<SystemTime>,
    /// Price zone (SE1, SE2, SE3, SE4)
    price_zone: String,
    /// Whether Tibber data is available
    tibber_available: bool,
}

/// A single price point with data from both sources
#[derive(Clone, Debug, Serialize)]
pub struct PricePoint {
    /// Start time of price period (ISO 8601)
    pub starts_at: String,
    /// End time of price period (ISO 8601)
    pub ends_at: String,

    // From elprisetjustnu.se (raw spot price)
    /// Spot price in SEK/kWh
    pub spot_sek: f64,
    /// Spot price in EUR/kWh
    pub spot_eur: f64,
    /// EUR->SEK exchange rate
    pub exchange_rate: f64,

    // From Tibber (if available)
    /// Tibber total price SEK/kWh (what you pay)
    pub tibber_total: Option<f64>,
    /// Tibber energy component SEK/kWh
    pub tibber_energy: Option<f64>,
    /// Tibber tax component SEK/kWh
    pub tibber_tax: Option<f64>,
    /// Price level from Tibber
    pub level: Option<PriceLevel>,

    // Calculated comparison
    /// Tibber markup: `tibber_total` - `spot_sek`
    pub markup: Option<f64>,
    /// Markup as percentage: (markup / `spot_sek`) * 100
    pub markup_percent: Option<f64>,
}

/// Price level classification
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriceLevel {
    VeryCheap,
    Cheap,
    Normal,
    Expensive,
    VeryExpensive,
}

/// Statistics for a set of prices
#[derive(Clone, Debug, Serialize)]
pub struct PriceStatistics {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
}

/// Analysis of Tibber markup over spot price
#[derive(Clone, Debug, Serialize)]
pub struct MarkupAnalysis {
    /// Average markup in SEK/kWh
    pub avg_markup_sek: f64,
    /// Average markup as percentage
    pub avg_markup_percent: f64,
    /// True if markup appears to be a fixed fee (low variance)
    pub is_fixed_fee: bool,
    /// True if markup appears to be percentage-based (high variance)
    pub is_percentage: bool,
    /// Estimated extra monthly cost at 1000 kWh/month
    pub estimated_monthly_cost: f64,
}

impl PriceState {
    /// Create a new price state
    pub fn new(price_zone: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PriceStateInner {
                current: None,
                today: Vec::new(),
                tomorrow: Vec::new(),
                last_updated: None,
                price_zone,
                tibber_available: false,
            })),
        }
    }

    /// Update prices from both sources
    pub fn update_prices(&self, today: Vec<PricePoint>, tomorrow: Vec<PricePoint>) {
        let mut inner = self.inner.lock().unwrap();

        // Find current price based on time
        let now = chrono::Utc::now();
        let current = today.iter().find(|p| {
            if let (Ok(start), Ok(end)) = (
                chrono::DateTime::parse_from_rfc3339(&p.starts_at),
                chrono::DateTime::parse_from_rfc3339(&p.ends_at),
            ) {
                now >= start && now < end
            } else {
                false
            }
        });

        inner.current = current.cloned();
        inner.today = today;
        inner.tomorrow = tomorrow;
        inner.last_updated = Some(SystemTime::now());
        inner.tibber_available = inner
            .today
            .first()
            .is_some_and(|p| p.tibber_total.is_some());
    }

    /// Get the current price point
    pub fn get_current(&self) -> Option<PricePoint> {
        let inner = self.inner.lock().unwrap();
        inner.current.clone()
    }

    /// Get today's prices
    pub fn get_today(&self) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();
        inner.today.clone()
    }

    /// Get tomorrow's prices (if available)
    pub fn get_tomorrow(&self) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();
        inner.tomorrow.clone()
    }

    /// Check if tomorrow's prices are available
    pub fn tomorrow_available(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.tomorrow.is_empty()
    }

    /// Check if Tibber data is available
    pub fn tibber_available(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.tibber_available
    }

    /// Get the price zone
    pub fn price_zone(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner.price_zone.clone()
    }

    /// Get last update time
    pub fn last_updated(&self) -> Option<SystemTime> {
        let inner = self.inner.lock().unwrap();
        inner.last_updated
    }

    /// Calculate statistics for spot prices
    #[must_use]
    pub fn get_spot_statistics(prices: &[PricePoint]) -> Option<PriceStatistics> {
        calculate_statistics(prices, |p| p.spot_sek)
    }

    /// Calculate statistics for Tibber prices
    #[must_use]
    pub fn get_tibber_statistics(prices: &[PricePoint]) -> Option<PriceStatistics> {
        let tibber_prices: Vec<f64> = prices.iter().filter_map(|p| p.tibber_total).collect();
        if tibber_prices.is_empty() {
            return None;
        }
        calculate_statistics_from_values(&tibber_prices)
    }

    /// Get optimal (cheapest) hours in the next 24h
    pub fn get_optimal_hours(&self, count: usize) -> Vec<PricePoint> {
        let inner = self.inner.lock().unwrap();

        // Combine today (remaining) and tomorrow prices
        let now = chrono::Utc::now();
        let mut future_prices: Vec<PricePoint> = inner
            .today
            .iter()
            .chain(inner.tomorrow.iter())
            .filter(|p| {
                chrono::DateTime::parse_from_rfc3339(&p.starts_at)
                    .is_ok_and(|start| start > now)
            })
            .cloned()
            .collect();

        // Sort by spot price (ascending)
        future_prices.sort_by(|a, b| {
            a.spot_sek
                .partial_cmp(&b.spot_sek)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        future_prices.into_iter().take(count).collect()
    }

    /// Analyze Tibber markup over spot price
    #[allow(clippy::cast_precision_loss)]
    pub fn analyze_markup(&self) -> Option<MarkupAnalysis> {
        let inner = self.inner.lock().unwrap();

        let markups: Vec<f64> = inner
            .today
            .iter()
            .chain(inner.tomorrow.iter())
            .filter_map(|p| p.markup)
            .collect();

        if markups.is_empty() {
            return None;
        }

        let markup_percents: Vec<f64> = inner
            .today
            .iter()
            .chain(inner.tomorrow.iter())
            .filter_map(|p| p.markup_percent)
            .collect();

        let avg_markup_sek = markups.iter().sum::<f64>() / markups.len() as f64;
        let avg_markup_percent = if markup_percents.is_empty() {
            0.0
        } else {
            markup_percents.iter().sum::<f64>() / markup_percents.len() as f64
        };

        // Calculate variance to determine if fixed fee or percentage
        let variance = calculate_variance(&markups);
        let is_fixed_fee = variance < 0.001; // Very low variance = fixed fee
        let is_percentage = variance > 0.01; // Higher variance = percentage-based

        // Estimate monthly cost at 1000 kWh/month
        let estimated_monthly_cost = avg_markup_sek * 1000.0;

        Some(MarkupAnalysis {
            avg_markup_sek,
            avg_markup_percent,
            is_fixed_fee,
            is_percentage,
            estimated_monthly_cost,
        })
    }
}

/// Calculate statistics from price points using a selector function
fn calculate_statistics<F>(prices: &[PricePoint], selector: F) -> Option<PriceStatistics>
where
    F: Fn(&PricePoint) -> f64,
{
    if prices.is_empty() {
        return None;
    }

    let values: Vec<f64> = prices.iter().map(&selector).collect();
    calculate_statistics_from_values(&values)
}

/// Calculate statistics from a list of values
#[allow(clippy::cast_precision_loss)]
fn calculate_statistics_from_values(values: &[f64]) -> Option<PriceStatistics> {
    if values.is_empty() {
        return None;
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let mean = values.iter().sum::<f64>() / values.len() as f64;

    let median = if sorted.len().is_multiple_of(2) {
        let middle = sorted.len() / 2;
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[sorted.len() / 2]
    };

    Some(PriceStatistics {
        min,
        max,
        mean,
        median,
    })
}

/// Calculate variance of a list of values
#[allow(clippy::cast_precision_loss)]
fn calculate_variance(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let sum_sq_diff: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
    sum_sq_diff / values.len() as f64
}

impl PriceLevel {
    /// Parse price level from Tibber API string
    pub fn from_tibber_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "VERY_CHEAP" => Some(Self::VeryCheap),
            "CHEAP" => Some(Self::Cheap),
            "NORMAL" => Some(Self::Normal),
            "EXPENSIVE" => Some(Self::Expensive),
            "VERY_EXPENSIVE" => Some(Self::VeryExpensive),
            _ => None,
        }
    }

    /// Calculate price level based on percentile within a price range
    pub fn from_percentile(percentile: f64) -> Self {
        if percentile < 0.25 {
            Self::VeryCheap
        } else if percentile < 0.40 {
            Self::Cheap
        } else if percentile < 0.60 {
            Self::Normal
        } else if percentile < 0.75 {
            Self::Expensive
        } else {
            Self::VeryExpensive
        }
    }
}

impl PricePoint {
    /// Create a new price point from spot data only
    pub fn from_spot(
        starts_at: String,
        ends_at: String,
        spot_sek: f64,
        spot_eur: f64,
        exchange_rate: f64,
    ) -> Self {
        Self {
            starts_at,
            ends_at,
            spot_sek,
            spot_eur,
            exchange_rate,
            tibber_total: None,
            tibber_energy: None,
            tibber_tax: None,
            level: None,
            markup: None,
            markup_percent: None,
        }
    }

    /// Merge Tibber data into this price point
    pub fn with_tibber(
        mut self,
        total: f64,
        energy: f64,
        tax: f64,
        level: Option<PriceLevel>,
    ) -> Self {
        self.tibber_total = Some(total);
        self.tibber_energy = Some(energy);
        self.tibber_tax = Some(tax);
        self.level = level;

        // Calculate markup
        self.markup = Some(total - self.spot_sek);
        if self.spot_sek.abs() > f64::EPSILON {
            self.markup_percent = Some((total - self.spot_sek) / self.spot_sek * 100.0);
        }

        self
    }

    /// Set calculated price level (when Tibber level not available)
    pub fn with_calculated_level(mut self, level: PriceLevel) -> Self {
        if self.level.is_none() {
            self.level = Some(level);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_float_eq(a: f64, b: f64, msg: &str) {
        assert!(
            (a - b).abs() < 0.0001,
            "{msg}: expected {b}, got {a}"
        );
    }

    #[test]
    fn test_price_state_new() {
        let state = PriceState::new("SE3".to_string());
        assert_eq!(state.price_zone(), "SE3");
        assert!(state.get_current().is_none());
        assert!(state.get_today().is_empty());
        assert!(!state.tomorrow_available());
    }

    #[test]
    fn test_price_point_from_spot() {
        let point = PricePoint::from_spot(
            "2026-01-04T14:00:00+01:00".to_string(),
            "2026-01-04T14:15:00+01:00".to_string(),
            0.72,
            0.065,
            11.08,
        );

        assert_float_eq(point.spot_sek, 0.72, "spot_sek");
        assert_float_eq(point.spot_eur, 0.065, "spot_eur");
        assert!(point.tibber_total.is_none());
        assert!(point.markup.is_none());
    }

    #[test]
    fn test_price_point_with_tibber() {
        let point = PricePoint::from_spot(
            "2026-01-04T14:00:00+01:00".to_string(),
            "2026-01-04T14:15:00+01:00".to_string(),
            0.72,
            0.065,
            11.08,
        )
        .with_tibber(0.85, 0.56, 0.17, Some(PriceLevel::Normal));

        assert_float_eq(point.tibber_total.unwrap(), 0.85, "tibber_total");
        assert_float_eq(point.markup.unwrap(), 0.13, "markup");
        assert_float_eq(point.markup_percent.unwrap(), 18.0555, "markup_percent");
        assert_eq!(point.level, Some(PriceLevel::Normal));
    }

    #[test]
    fn test_price_level_from_tibber_str() {
        assert_eq!(
            PriceLevel::from_tibber_str("VERY_CHEAP"),
            Some(PriceLevel::VeryCheap)
        );
        assert_eq!(
            PriceLevel::from_tibber_str("normal"),
            Some(PriceLevel::Normal)
        );
        assert_eq!(PriceLevel::from_tibber_str("INVALID"), None);
    }

    #[test]
    fn test_price_level_from_percentile() {
        assert_eq!(PriceLevel::from_percentile(0.10), PriceLevel::VeryCheap);
        assert_eq!(PriceLevel::from_percentile(0.30), PriceLevel::Cheap);
        assert_eq!(PriceLevel::from_percentile(0.50), PriceLevel::Normal);
        assert_eq!(PriceLevel::from_percentile(0.70), PriceLevel::Expensive);
        assert_eq!(PriceLevel::from_percentile(0.90), PriceLevel::VeryExpensive);
    }

    #[test]
    fn test_calculate_statistics() {
        let prices = vec![
            PricePoint::from_spot(String::new(), String::new(), 0.5, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 1.0, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 0.75, 0.0, 0.0),
            PricePoint::from_spot(String::new(), String::new(), 1.25, 0.0, 0.0),
        ];

        let stats = calculate_statistics(&prices, |p| p.spot_sek).unwrap();
        assert_float_eq(stats.min, 0.5, "min");
        assert_float_eq(stats.max, 1.25, "max");
        assert_float_eq(stats.mean, 0.875, "mean");
        assert_float_eq(stats.median, 0.875, "median"); // (0.75 + 1.0) / 2
    }

    #[test]
    fn test_calculate_variance() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let variance = calculate_variance(&values);
        assert_float_eq(variance, 2.0, "variance");
    }
}
