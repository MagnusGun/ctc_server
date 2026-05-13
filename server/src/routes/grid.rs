//! Grid information API endpoints
//!
//! Provides tariff mode, current hour consumption, monthly peak tracking, and electricity prices.

use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::energy::grid::{GridState, PeakHour};
use crate::energy::price::{PricePoint, PriceState, PriceStatistics};
use crate::energy::tariff::{TariffMode, get_current_tariff};
use crate::error::ApiError;

/// State for grid routes
#[derive(Clone)]
pub struct GridRouteState {
    pub grid_state: GridState,
    pub price_state: PriceState,
}

pub fn routes(grid_state: GridState, price_state: PriceState) -> Router {
    let state = GridRouteState {
        grid_state,
        price_state,
    };

    Router::new()
        .route("/api/v1/grid", get(get_grid_status))
        .route("/api/v1/grid/tariff", get(get_tariff))
        .route("/api/v1/prices", get(get_prices))
        .route("/api/v1/prices/current", get(get_current_price))
        .route("/api/v1/prices/optimal", get(get_optimal_hours))
        .with_state(state)
}

/// Full grid status response
#[derive(Serialize)]
struct GridResponse {
    /// Current tariff mode (high/low)
    tariff_mode: TariffMode,
    /// Current 15-minute period's accumulated consumption in kWh (from Tibber)
    current_quarter_kwh: f64,
    /// Recent 15-minute consumption history (rolling 24 h, oldest first)
    consumption_15min: Vec<crate::energy::grid::ConsumptionEntry>,
    /// Average of the 3 highest daily peaks this month in kWh
    monthly_peak_avg_kwh: f64,
    /// The top 3 peak hours this month (one per day)
    monthly_peak_hours: Vec<PeakHour>,
    /// Number of hours recorded this month
    recorded_hours: usize,
    /// Number of unique days with recorded data
    recorded_days: usize,
}

/// Tariff-only response
#[derive(Serialize)]
struct TariffResponse {
    /// Current tariff mode (high/low)
    tariff_mode: TariffMode,
    /// Human-readable Swedish description
    description: String,
}

/// Get full grid status
/// GET /api/v1/grid
///
/// Returns tariff mode, current hour consumption, and monthly peak data.
async fn get_grid_status(State(state): State<GridRouteState>) -> Result<String, ApiError> {
    debug!("get_grid_status: START");

    let tariff_mode = get_current_tariff();

    // Current 15-minute period — populated from the WebSocket-driven
    // accumulated_consumption delta. `consumption_15min` is the rolling 24 h
    // of completed quarters, oldest first.
    let current_quarter_kwh = state.grid_state.get_current_quarter_kwh();
    let consumption_15min = state.grid_state.get_consumption_15min();

    let monthly_peak_avg_kwh = state.grid_state.get_top3_average();
    let monthly_peak_hours = state.grid_state.get_top3_hours();
    let recorded_hours = state.grid_state.recorded_hours_count();
    let recorded_days = state.grid_state.get_recorded_days_count();

    let response = GridResponse {
        tariff_mode,
        current_quarter_kwh,
        consumption_15min,
        monthly_peak_avg_kwh,
        monthly_peak_hours,
        recorded_hours,
        recorded_days,
    };

    debug!(
        "get_grid_status: tariff={}, current_quarter={}, peak_avg={}",
        response.tariff_mode, response.current_quarter_kwh, response.monthly_peak_avg_kwh
    );

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_grid_status: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

/// Get current tariff mode only
/// GET /api/v1/grid/tariff
///
/// Returns just the tariff mode for lightweight polling.
async fn get_tariff() -> Result<String, ApiError> {
    debug!("get_tariff: START");

    let tariff_mode = get_current_tariff();
    let description = match tariff_mode {
        TariffMode::High => "Högtariff - dyrare elpris".to_string(),
        TariffMode::Low => "Lågtariff - billigare elpris".to_string(),
    };

    let response = TariffResponse {
        tariff_mode,
        description,
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_tariff: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

// ==================== Price Endpoints ====================

/// Query parameters for optimal hours
#[derive(Deserialize)]
struct OptimalHoursQuery {
    /// Number of optimal hours to return (default: 3)
    hours: Option<usize>,
}

/// Full price response with all data
#[derive(Serialize)]
struct PriceResponse {
    /// Current price point
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<PricePoint>,
    /// Today's price data
    today: DayPrices,
    /// Tomorrow's price data
    tomorrow: DayPrices,
    /// Optimal (cheapest) hours in next 24h
    optimal_hours: Vec<PricePoint>,
    /// Price trend: "rising", "falling", or "stable"
    trend: String,
    /// Price zone (SE1-SE4)
    price_zone: String,
}

/// Prices for a single day with statistics
#[derive(Serialize)]
struct DayPrices {
    /// Price points for the day
    prices: Vec<PricePoint>,
    /// Whether prices are available
    available: bool,
    /// Spot price statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    spot_statistics: Option<PriceStatistics>,
}

/// Lightweight current price response
#[derive(Serialize)]
struct CurrentPriceResponse {
    /// Current price point
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<PricePoint>,
    /// Price zone
    price_zone: String,
}

/// Optimal hours response
#[derive(Serialize)]
struct OptimalHoursResponse {
    /// Optimal hours sorted by price
    hours: Vec<PricePoint>,
    /// Number of hours requested
    count: usize,
}

/// Get all price information
/// GET /api/v1/prices
///
/// Returns current price, 48h prices (today + tomorrow), statistics, and analysis.
async fn get_prices(State(state): State<GridRouteState>) -> Result<String, ApiError> {
    debug!("get_prices: START");

    let today = state.price_state.get_today();
    let tomorrow = state.price_state.get_tomorrow();
    let current = state.price_state.get_current();
    let price_zone = state.price_state.price_zone();

    // Calculate statistics
    let today_spot_stats = PriceState::get_spot_statistics(&today);
    let tomorrow_spot_stats = PriceState::get_spot_statistics(&tomorrow);

    // Get optimal hours
    let optimal_hours = state.price_state.get_optimal_hours(3);

    // Calculate trend
    let trend = calculate_price_trend(&today);

    debug!("get_prices: zone={}", price_zone);

    let response = PriceResponse {
        current,
        today: DayPrices {
            prices: today,
            available: true,
            spot_statistics: today_spot_stats,
        },
        tomorrow: DayPrices {
            prices: tomorrow.clone(),
            available: !tomorrow.is_empty(),
            spot_statistics: tomorrow_spot_stats,
        },
        optimal_hours,
        trend,
        price_zone,
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_prices: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

/// Get current price only
/// GET /api/v1/prices/current
///
/// Returns just the current price for lightweight polling.
async fn get_current_price(State(state): State<GridRouteState>) -> Result<String, ApiError> {
    debug!("get_current_price: START");

    let response = CurrentPriceResponse {
        current: state.price_state.get_current(),
        price_zone: state.price_state.price_zone(),
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_current_price: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

/// Get optimal (cheapest) hours
/// GET /api/v1/prices/optimal?hours=3
///
/// Returns the cheapest hours in the next 24h.
async fn get_optimal_hours(
    State(state): State<GridRouteState>,
    Query(query): Query<OptimalHoursQuery>,
) -> Result<String, ApiError> {
    debug!("get_optimal_hours: START");

    let count = query.hours.unwrap_or(3).min(24); // Cap at 24 hours
    let hours = state.price_state.get_optimal_hours(count);

    // Report the actual number of slots returned, not the capped request —
    // the caller wants to know how much data they got.
    let response = OptimalHoursResponse {
        count: hours.len(),
        hours,
    };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_optimal_hours: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

/// Calculate price trend based on today's prices
///
/// Requires at least 8 price points so each quarter averages a minimum of 2
/// entries — comparing single hours would give an unreliable trend signal.
#[allow(clippy::cast_precision_loss)]
fn calculate_price_trend(prices: &[PricePoint]) -> String {
    if prices.len() < 8 {
        return "unknown".to_string();
    }

    // Compare first quarter vs last quarter of prices
    let quarter = prices.len() / 4;
    let first_avg: f64 = prices[..quarter].iter().map(|p| p.spot_sek).sum::<f64>() / quarter as f64;
    let last_avg: f64 = prices[prices.len() - quarter..]
        .iter()
        .map(|p| p.spot_sek)
        .sum::<f64>()
        / quarter as f64;

    // Dividing by `first_avg` is unreliable when it's zero (NaN) or negative
    // (sign flips comparisons). Spot prices CAN be negative at Nord Pool.
    // Fall back to absolute change in SEK/kWh with a small threshold.
    if first_avg <= 0.0 {
        let diff = last_avg - first_avg;
        return if diff > 0.05 {
            "rising".to_string()
        } else if diff < -0.05 {
            "falling".to_string()
        } else {
            "stable".to_string()
        };
    }

    let change_percent = (last_avg - first_avg) / first_avg * 100.0;

    if change_percent > 10.0 {
        "rising".to_string()
    } else if change_percent < -10.0 {
        "falling".to_string()
    } else {
        "stable".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_state() -> GridRouteState {
        GridRouteState {
            grid_state: GridState::new(),
            price_state: PriceState::new("SE3".to_string()),
        }
    }

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("response is valid JSON")
    }

    #[tokio::test]
    async fn test_get_tariff() {
        let json = get_tariff().await.expect("get_tariff");
        let v = parse(&json);
        assert!(v["tariff_mode"].is_string());
        assert!(v["description"].is_string());
    }

    #[tokio::test]
    async fn test_get_grid_status() {
        let state = create_test_state();

        let json = get_grid_status(State(state)).await.expect("get_grid_status");
        let v = parse(&json);
        assert!(v["tariff_mode"].is_string());
        // monthly_peak_avg_kwh starts at 0 with no recorded peaks.
        assert!(v["monthly_peak_avg_kwh"].is_number());
    }

    #[tokio::test]
    async fn test_get_prices_empty() {
        let state = create_test_state();

        let json = get_prices(State(state)).await.expect("get_prices");
        let v = parse(&json);
        assert_eq!(v["price_zone"].as_str(), Some("SE3"));
        assert!(v["today"].is_object());
        assert!(v["today"]["prices"].is_array());
        assert!(v["tomorrow"].is_object());
        assert!(v["tomorrow"]["prices"].is_array());
    }

    #[tokio::test]
    async fn test_get_current_price() {
        let state = create_test_state();

        let json = get_current_price(State(state)).await.expect("get_current_price");
        let v = parse(&json);
        assert_eq!(v["price_zone"].as_str(), Some("SE3"));
    }

    #[tokio::test]
    async fn test_get_optimal_hours() {
        let state = create_test_state();

        let json = get_optimal_hours(State(state), Query(OptimalHoursQuery { hours: Some(3) }))
            .await
            .expect("get_optimal_hours");
        let v = parse(&json);
        assert!(v["hours"].is_array());
        assert!(v["count"].is_u64());
    }

    #[test]
    fn test_calculate_price_trend_unknown() {
        let prices: Vec<PricePoint> = vec![];
        assert_eq!(calculate_price_trend(&prices), "unknown");

        let prices = vec![PricePoint::from_spot(
            String::new(),
            String::new(),
            1.0,
            0.0,
            0.0,
        )];
        assert_eq!(calculate_price_trend(&prices), "unknown");
    }

    #[test]
    fn test_calculate_price_trend_stable() {
        // Create prices that fluctuate around 1.0 but don't trend significantly
        let prices: Vec<PricePoint> = (0..24)
            .map(|i| {
                // Oscillate between 0.95 and 1.05
                let value = 1.0 + (if i % 2 == 0 { 0.03 } else { -0.03 });
                PricePoint::from_spot(String::new(), String::new(), value, 0.0, 0.0)
            })
            .collect();
        assert_eq!(calculate_price_trend(&prices), "stable");
    }

    #[test]
    fn test_calculate_price_trend_rising() {
        let prices: Vec<PricePoint> = (0..24)
            .map(|i| {
                PricePoint::from_spot(
                    String::new(),
                    String::new(),
                    0.5 + (f64::from(i) * 0.05),
                    0.0,
                    0.0,
                )
            })
            .collect();
        assert_eq!(calculate_price_trend(&prices), "rising");
    }

    #[test]
    fn test_calculate_price_trend_falling() {
        let prices: Vec<PricePoint> = (0..24)
            .map(|i| {
                PricePoint::from_spot(
                    String::new(),
                    String::new(),
                    1.5 - (f64::from(i) * 0.05),
                    0.0,
                    0.0,
                )
            })
            .collect();
        assert_eq!(calculate_price_trend(&prices), "falling");
    }

    #[test]
    fn test_calculate_price_trend_negative_first_rising() {
        // First quarter averages -0.05, last quarter averages 0.20 — clearly
        // rising. Percentage math against negative would flip the sign.
        let mut prices: Vec<PricePoint> = (0..6)
            .map(|_| PricePoint::from_spot(String::new(), String::new(), -0.05, 0.0, 0.0))
            .collect();
        prices.extend(
            (0..18).map(|_| PricePoint::from_spot(String::new(), String::new(), 0.20, 0.0, 0.0)),
        );
        assert_eq!(calculate_price_trend(&prices), "rising");
    }

    #[test]
    fn test_calculate_price_trend_zero_first_stable() {
        // First quarter averages 0.0, last quarter also 0.0 — stable, not NaN.
        let prices: Vec<PricePoint> = (0..24)
            .map(|_| PricePoint::from_spot(String::new(), String::new(), 0.0, 0.0, 0.0))
            .collect();
        assert_eq!(calculate_price_trend(&prices), "stable");
    }
}
