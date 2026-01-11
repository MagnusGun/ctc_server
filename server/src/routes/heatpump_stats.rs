//! Heat pump statistics API endpoints
//!
//! Provides compressor cycle statistics including:
//! - Cycle times (min/max/avg)
//! - Compressor starts per time window
//! - Operating hours per time window
//! - Historical data for charts

use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use serde::Deserialize;
use tracing::debug;

use crate::error::ApiError;
use crate::heatpump::stats::HeatPumpStats;

/// Query parameters for history endpoint
#[derive(Deserialize)]
struct HistoryQuery {
    /// Number of days of history to return (default: 30, max: 365)
    days: Option<usize>,
}

pub fn routes(stats: HeatPumpStats) -> Router {
    Router::new()
        .route("/api/v1/heatpump/stats", get(get_stats))
        .route("/api/v1/heatpump/stats/history", get(get_history))
        .with_state(stats)
}

/// Get current heat pump statistics
/// GET /api/v1/heatpump/stats
///
/// Returns cycle time statistics, starts per window, operating hours per window,
/// and tracking metadata.
async fn get_stats(State(stats): State<HeatPumpStats>) -> Result<String, ApiError> {
    debug!("get_heatpump_stats: START");

    let response = stats.get_summary();

    debug!(
        "get_heatpump_stats: compressor_on={}, total_starts={}",
        response.compressor_on, response.tracking.total_starts
    );

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_heatpump_stats: JSON error - {}", e);
            ApiError::InternalError
        })
}

/// Get historical data for charts
/// GET /api/v1/heatpump/stats/history?days=30
///
/// Returns recent cycles and daily aggregated statistics for rendering charts.
async fn get_history(
    State(stats): State<HeatPumpStats>,
    Query(query): Query<HistoryQuery>,
) -> Result<String, ApiError> {
    debug!("get_heatpump_history: START");

    let days = query.days.unwrap_or(30).min(365);
    let response = stats.get_history(days);

    debug!(
        "get_heatpump_history: cycles={}, daily={}",
        response.cycles.len(),
        response.daily.len()
    );

    serde_json::to_string(&response)
        .map(|s| format!("{s}\n"))
        .map_err(|e| {
            tracing::error!("get_heatpump_history: JSON error - {}", e);
            ApiError::InternalError
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_stats() {
        let stats = HeatPumpStats::new();

        let result = get_stats(State(stats)).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("compressor_on"));
        assert!(json.contains("starts"));
        assert!(json.contains("operating_hours"));
        assert!(json.contains("tracking"));
    }

    #[tokio::test]
    async fn test_get_stats_with_cycles() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first, then simulate a complete cycle
        stats.update_state(0, Some(0.0)); // Initialize
        stats.update_state(3, Some(-5.0)); // Compressor ON (observed start)
        stats.update_state(0, Some(-5.0)); // Compressor OFF (cycle complete)

        let result = get_stats(State(stats)).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("cycle_stats"));
        assert!(json.contains("min_secs"));
        assert!(json.contains("max_secs"));
    }

    #[tokio::test]
    async fn test_get_history_default_days() {
        let stats = HeatPumpStats::new();

        let result = get_history(State(stats), Query(HistoryQuery { days: None })).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("cycles"));
        assert!(json.contains("daily"));
    }

    #[tokio::test]
    async fn test_get_history_custom_days() {
        let stats = HeatPumpStats::new();

        let result = get_history(State(stats), Query(HistoryQuery { days: Some(7) })).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        assert!(json.contains("cycles"));
        assert!(json.contains("daily"));
    }

    #[tokio::test]
    async fn test_get_history_max_days_capped() {
        let stats = HeatPumpStats::new();

        // Request 1000 days - should be capped at 365
        let result = get_history(State(stats), Query(HistoryQuery { days: Some(1000) })).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_history_with_cycles() {
        let stats = HeatPumpStats::new();

        // Create some cycles
        for temp in [-10.0, -5.0, 0.0] {
            stats.update_state(3, Some(temp));
            stats.update_state(0, Some(temp));
        }

        let result = get_history(State(stats), Query(HistoryQuery { days: Some(30) })).await;
        assert!(result.is_ok());

        let json = result.unwrap();
        // Should have 3 cycles in history
        assert!(json.contains("duration_secs"));
        assert!(json.contains("outdoor_temp_c"));
    }
}
