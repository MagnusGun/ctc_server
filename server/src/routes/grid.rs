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
use crate::energy::price::{MarkupAnalysis, PricePoint, PriceState, PriceStatistics};
use crate::energy::tariff::{TariffMode, get_current_tariff};
use crate::error::ApiError;

/// State for grid routes
#[derive(Clone)]
pub struct GridRouteState {
    pub grid_state: GridState,
    pub price_state: PriceState,
    pub tibber_enabled: bool,
}

pub fn routes(grid_state: GridState, price_state: PriceState, tibber_enabled: bool) -> Router {
    let state = GridRouteState {
        grid_state,
        price_state,
        tibber_enabled,
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
    /// Current hour's accumulated consumption in kWh (from Tibber)
    #[serde(skip_serializing_if = "Option::is_none")]
    current_hour_kwh: Option<f64>,
    /// Average of the 3 highest daily peaks this month in kWh
    monthly_peak_avg_kwh: f64,
    /// The top 3 peak hours this month (one per day)
    monthly_peak_hours: Vec<PeakHour>,
    /// Number of hours recorded this month
    recorded_hours: usize,
    /// Number of unique days with recorded data
    recorded_days: usize,
    /// Whether Tibber is configured and available
    tibber_available: bool,
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

    // Get current hour consumption from grid state (populated by WebSocket)
    let local_kwh = state.grid_state.get_current_hour_kwh();
    let current_hour_kwh = if local_kwh > 0.0 {
        Some(local_kwh)
    } else {
        None
    };
    let tibber_available = state.tibber_enabled;

    let monthly_peak_avg_kwh = state.grid_state.get_top3_average();
    let monthly_peak_hours = state.grid_state.get_top3_hours();
    let recorded_hours = state.grid_state.recorded_hours_count();
    let recorded_days = state.grid_state.get_recorded_days_count();

    let response = GridResponse {
        tariff_mode,
        current_hour_kwh,
        monthly_peak_avg_kwh,
        monthly_peak_hours,
        recorded_hours,
        recorded_days,
        tibber_available,
    };

    debug!(
        "get_grid_status: tariff={}, current_hour={:?}, peak_avg={}",
        response.tariff_mode, response.current_hour_kwh, response.monthly_peak_avg_kwh
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
    /// Tibber markup analysis
    #[serde(skip_serializing_if = "Option::is_none")]
    markup_analysis: Option<MarkupAnalysis>,
    /// Optimal (cheapest) hours in next 24h
    optimal_hours: Vec<PricePoint>,
    /// Price trend: "rising", "falling", or "stable"
    trend: String,
    /// Whether Tibber data is available
    tibber_available: bool,
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
    /// Tibber price statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    tibber_statistics: Option<PriceStatistics>,
}

/// Lightweight current price response
#[derive(Serialize)]
struct CurrentPriceResponse {
    /// Current price point
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<PricePoint>,
    /// Whether Tibber data is available
    tibber_available: bool,
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
    let tibber_available = state.price_state.tibber_available();
    let price_zone = state.price_state.price_zone();

    // Calculate statistics
    let today_spot_stats = PriceState::get_spot_statistics(&today);
    let today_tibber_stats = PriceState::get_tibber_statistics(&today);
    let tomorrow_spot_stats = PriceState::get_spot_statistics(&tomorrow);
    let tomorrow_tibber_stats = PriceState::get_tibber_statistics(&tomorrow);

    // Get optimal hours
    let optimal_hours = state.price_state.get_optimal_hours(3);

    // Analyze markup
    let markup_analysis = state.price_state.analyze_markup();

    // Calculate trend
    let trend = calculate_price_trend(&today);

    let response = PriceResponse {
        current,
        today: DayPrices {
            prices: today,
            available: true,
            spot_statistics: today_spot_stats,
            tibber_statistics: today_tibber_stats,
        },
        tomorrow: DayPrices {
            prices: tomorrow.clone(),
            available: !tomorrow.is_empty(),
            spot_statistics: tomorrow_spot_stats,
            tibber_statistics: tomorrow_tibber_stats,
        },
        markup_analysis,
        optimal_hours,
        trend,
        tibber_available,
        price_zone,
    };

    debug!("get_prices: tibber_available={}", tibber_available);

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
        tibber_available: state.price_state.tibber_available(),
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

    let response = OptimalHoursResponse { hours, count };

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_optimal_hours: JSON serialization error - {}", e);
            ApiError::InternalError
        })
}

/// Calculate price trend based on today's prices
#[allow(clippy::cast_precision_loss)]
fn calculate_price_trend(prices: &[PricePoint]) -> String {
    if prices.len() < 4 {
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
            tibber_enabled: false,
        }
    }

    #[tokio::test]
    async fn test_get_tariff() {
        let result = get_tariff().await;
        assert!(result.is_ok());

        let json = result.unwrap();
        // Should contain tariff_mode field
        assert!(json.contains("tariff_mode"));
        assert!(json.contains("description"));
    }

    #[tokio::test]
    async fn test_get_grid_status_no_tibber() {
        let state = create_test_state();

        let result = get_grid_status(State(state)).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("tariff_mode"));
        assert!(json.contains("monthly_peak_avg_kwh"));
        assert!(json.contains("tibber_available"));
        assert!(json.contains("false")); // tibber_available should be false
    }

    #[tokio::test]
    async fn test_get_prices_empty() {
        let state = create_test_state();

        let result = get_prices(State(state)).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("today"));
        assert!(json.contains("tomorrow"));
        assert!(json.contains("price_zone"));
        assert!(json.contains("SE3"));
    }

    #[tokio::test]
    async fn test_get_current_price() {
        let state = create_test_state();

        let result = get_current_price(State(state)).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("tibber_available"));
        assert!(json.contains("price_zone"));
    }

    #[tokio::test]
    async fn test_get_optimal_hours() {
        let state = create_test_state();

        let result =
            get_optimal_hours(State(state), Query(OptimalHoursQuery { hours: Some(3) })).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("hours"));
        assert!(json.contains("count"));
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
}
