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

    let days = query.days.unwrap_or(30).clamp(1, 365);
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

    fn parse(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("response is valid JSON")
    }

    #[tokio::test]
    async fn test_get_stats() {
        let stats = HeatPumpStats::new();

        let json = get_stats(State(stats)).await.expect("get_stats");
        let v = parse(&json);

        assert_eq!(v["compressor_on"].as_bool(), Some(false));
        assert_eq!(v["starts"]["this_hour"].as_u64(), Some(0));
        assert_eq!(v["starts"]["this_day"].as_u64(), Some(0));
        assert_eq!(v["operating_hours"]["this_hour"].as_f64(), Some(0.0));
        assert!(v["tracking"]["started_at"].is_string());
    }

    #[tokio::test]
    async fn test_get_stats_with_cycles() {
        let stats = HeatPumpStats::new();

        // Initialize with OFF state first, then simulate a complete cycle
        stats.update_state(0, Some(0.0)); // Initialize
        stats.update_state(3, Some(-5.0)); // Compressor ON (observed start)
        stats.update_state(0, Some(-5.0)); // Compressor OFF (cycle complete)

        let json = get_stats(State(stats)).await.expect("get_stats");
        let v = parse(&json);

        let cycle_stats = &v["cycle_stats"];
        assert!(cycle_stats.is_object(), "cycle_stats should be populated");
        assert!(cycle_stats["min_secs"].is_u64());
        assert!(cycle_stats["max_secs"].is_u64());
        assert_eq!(cycle_stats["cycle_count"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn test_get_history_default_days() {
        let stats = HeatPumpStats::new();

        let json = get_history(State(stats), Query(HistoryQuery { days: None }))
            .await
            .expect("get_history");
        let v = parse(&json);

        assert!(v["cycles"].is_array());
        assert!(v["daily"].is_array());
    }

    #[tokio::test]
    async fn test_get_history_custom_days() {
        let stats = HeatPumpStats::new();

        let json = get_history(State(stats), Query(HistoryQuery { days: Some(7) }))
            .await
            .expect("get_history");
        let v = parse(&json);

        assert!(v["cycles"].is_array());
        assert!(v["daily"].is_array());
    }

    #[tokio::test]
    async fn test_get_history_max_days_capped() {
        let stats = HeatPumpStats::new();

        // Request 1000 days - should be capped at 365 (no panic, returns ok)
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

        let json = get_history(State(stats), Query(HistoryQuery { days: Some(30) }))
            .await
            .expect("get_history");
        let v = parse(&json);

        let cycles = v["cycles"].as_array().expect("cycles array");
        // 2 of 3 iterations record: the first 3→0 transition is preceded by
        // initialization (not an observed start), so the partial cycle is
        // skipped. The remaining two iterations both record full cycles.
        assert_eq!(cycles.len(), 2);
        for c in cycles {
            assert!(c["duration_secs"].is_u64());
            // outdoor_temp_c is Option<f32>; tolerate both null and numeric.
            assert!(c["outdoor_temp_c"].is_number() || c["outdoor_temp_c"].is_null());
        }
    }
}
