//! Modbus telemetry endpoint.
//!
//! Surfaces a single JSON document with the actor's in-memory telemetry
//! counters plus the supervisor's process-lifetime counters. Intended for
//! tuning the bus configuration (`operation_timeout_secs`,
//! `inter_request_gap_ms`, retry pattern) — not as a general-purpose
//! monitoring dashboard. The actor's stats reset on respawn; the
//! supervisor's do not.

use std::time::Duration;

use axum::{Router, extract::State, routing::get};
use tracing::error;

use crate::error::ApiError;
use crate::modbus::ModbusSender;
use crate::modbus::operations::request_stats;

/// Route state: just the actor sender and the per-request timeout.
#[derive(Clone)]
pub struct ModbusStatsState {
    sender: ModbusSender,
    request_timeout_secs: u64,
}

pub fn routes(sender: ModbusSender, request_timeout_secs: u64) -> Router {
    let state = ModbusStatsState {
        sender,
        request_timeout_secs,
    };

    Router::new()
        .route("/api/v1/modbus/stats", get(get_modbus_stats))
        .with_state(state)
}

/// GET /api/v1/modbus/stats
///
/// Returns the actor's telemetry snapshot. Trips no Modbus I/O —
/// everything is in-process state.
async fn get_modbus_stats(State(route_state): State<ModbusStatsState>) -> Result<String, ApiError> {
    let stats = request_stats(
        &route_state.sender,
        Duration::from_secs(route_state.request_timeout_secs),
    )
    .await?;

    serde_json::to_string(stats.as_ref())
        .map(|s| s + "\n")
        .map_err(|e| {
            error!("get_modbus_stats: Failed to serialize stats: {e}");
            ApiError::InternalError
        })
}
