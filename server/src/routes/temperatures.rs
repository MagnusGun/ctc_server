use std::time::Duration;

use crate::config::TemperatureValidationConfig;
use crate::error::ApiError;
use crate::modbus::bms_parameters::{
    CTC_OUTDOOR_TEMP, CTC_RETURN_TEMP, CTC_ROOM_TEMP, HEATSYSTEM_FLOW_TEMP, HEATSYSTEM_ROOM_SETTEMP,
};
use crate::modbus::{ModbusSender, read_parameter, write_parameter};
use crate::storage::{Sensor, Store};
use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;

type TempState = (Store, ModbusSender, TemperatureValidationConfig, u64);

pub fn routes(
    store: Store,
    sender: ModbusSender,
    validation_config: TemperatureValidationConfig,
    request_timeout_secs: u64,
) -> Router {
    Router::new()
        .route("/api/v1/temperature/room", get(get_room_temp))
        .route("/api/v1/temperature/room/setpoint", get(get_room_set_temp))
        .route(
            "/api/v1/temperature/room/setpoint/",
            post(set_room_set_temp),
        )
        .route("/api/v1/temperature/outdoor", get(get_outdoor_temp))
        .route("/api/v1/temperature/flow", get(get_flow_temp))
        .route("/api/v1/temperature/flow/return", get(get_return_temp))
        .with_state((store, sender, validation_config, request_timeout_secs))
}

/// Format a cached sample using the same JSON shape as `read_parameter`.
/// `serde_json` handles f32 formatting consistently across toolchains; the
/// previous `{value:?}` Debug-format gave unstable precision.
fn format_cached(json_key: &str, value: f32) -> String {
    let body = serde_json::json!({ json_key: value });
    format!("{body}\n")
}

// read the actual room temperature
async fn get_room_temp(
    State((store, tx, _, timeout_secs)): State<TempState>,
) -> Result<String, ApiError> {
    if let Some((_, v)) = store.latest_sample(Sensor::Room) {
        return Ok(format_cached("room_temperature", v));
    }
    read_parameter(
        &tx,
        CTC_ROOM_TEMP,
        "room_temperature",
        "get_room_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

// read the room temperature setpoint
// Not cached — setpoints are user-controlled and not in the poller's set.
async fn get_room_set_temp(
    State((_store, tx, _, timeout_secs)): State<TempState>,
) -> Result<String, ApiError> {
    read_parameter(
        &tx,
        HEATSYSTEM_ROOM_SETTEMP,
        "room_temperature_setpoint",
        "get_room_set_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

#[derive(Debug, Deserialize)]
struct RoomSetPoint {
    value: f32,
}

// sets the room temperature setpoint
async fn set_room_set_temp(
    State((_store, tx, config, timeout_secs)): State<TempState>,
    Query(param): Query<RoomSetPoint>,
) -> Result<String, ApiError> {
    if param.value < config.min || param.value > config.max {
        return Err(ApiError::BadRequest);
    }

    write_parameter(
        &tx,
        HEATSYSTEM_ROOM_SETTEMP,
        param.value,
        "room_temperature_setpoint",
        "set_room_set_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

// read the outdoor temperature
async fn get_outdoor_temp(
    State((store, tx, _, timeout_secs)): State<TempState>,
) -> Result<String, ApiError> {
    if let Some((_, v)) = store.latest_sample(Sensor::Outdoor) {
        return Ok(format_cached("outdoor_temperature", v));
    }
    read_parameter(
        &tx,
        CTC_OUTDOOR_TEMP,
        "outdoor_temperature",
        "get_outdoor_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

// read heatpump outgoing flow temperature
async fn get_flow_temp(
    State((store, tx, _, timeout_secs)): State<TempState>,
) -> Result<String, ApiError> {
    if let Some((_, v)) = store.latest_sample(Sensor::Flow) {
        return Ok(format_cached("flow_outlet_temperature", v));
    }
    read_parameter(
        &tx,
        HEATSYSTEM_FLOW_TEMP,
        "flow_outlet_temperature",
        "get_flow_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

// read heatpump incoming flow temperature
async fn get_return_temp(
    State((store, tx, _, timeout_secs)): State<TempState>,
) -> Result<String, ApiError> {
    if let Some((_, v)) = store.latest_sample(Sensor::Return) {
        return Ok(format_cached("flow_return_temperature", v));
    }
    read_parameter(
        &tx,
        CTC_RETURN_TEMP,
        "flow_return_temperature",
        "get_return_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use tokio::sync::mpsc;

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();
        (dir, store)
    }

    fn dummy_state(store: Store) -> TempState {
        // The Modbus channel's receiver is dropped immediately. The cache
        // tests never reach the modbus path; if they did, the send would
        // fail and surface as ServiceUnavailable, which makes the test fail
        // loudly rather than hang.
        let (tx, _rx) = mpsc::channel(1);
        (
            store,
            tx,
            TemperatureValidationConfig {
                min: 10.0,
                max: 30.0,
            },
            1,
        )
    }

    #[tokio::test]
    async fn get_room_temp_returns_cached_value() {
        let (_dir, store) = tmp_store();
        store
            .record_sample(Sensor::Room, SystemTime::now(), 21.5)
            .unwrap();
        let result = get_room_temp(State(dummy_state(store))).await.unwrap();
        assert!(result.contains("\"room_temperature\":"), "got: {result}");
        assert!(result.contains("21.5"), "got: {result}");
    }

    #[tokio::test]
    async fn get_outdoor_temp_returns_cached_value() {
        let (_dir, store) = tmp_store();
        store
            .record_sample(Sensor::Outdoor, SystemTime::now(), -3.2)
            .unwrap();
        let result = get_outdoor_temp(State(dummy_state(store))).await.unwrap();
        assert!(result.contains("\"outdoor_temperature\":"), "got: {result}");
        assert!(result.contains("-3.2"), "got: {result}");
    }
}
