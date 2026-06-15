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
    use crate::modbus::test_support::spawn_fake_actor;
    use std::collections::HashMap;
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

    /// State whose Modbus sender is a fake actor seeded with `reads`
    /// (register id -> raw u16). The store is empty so handlers miss the
    /// cache and exercise the real Modbus read path.
    fn fake_state(store: Store, reads: HashMap<u16, u16>) -> TempState {
        (
            store,
            spawn_fake_actor(reads),
            TemperatureValidationConfig {
                min: 10.0,
                max: 30.0,
            },
            1,
        )
    }

    /// State whose Modbus sender's receiver is dropped, so any actual send
    /// fails and surfaces as `ServiceUnavailable`.
    fn closed_state(store: Store) -> TempState {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
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

    // --- Modbus read success paths (cache miss -> fake actor) ---

    #[tokio::test]
    async fn get_room_temp_reads_from_modbus_on_cache_miss() {
        let (_dir, store) = tmp_store();
        let mut reads = HashMap::new();
        // factor 0.1: raw 215 -> 21.5
        reads.insert(CTC_ROOM_TEMP.id, 215_u16);
        let result = get_room_temp(State(fake_state(store, reads)))
            .await
            .unwrap();
        assert!(result.contains("\"room_temperature\":"), "got: {result}");
        assert!(result.contains("21.5"), "got: {result}");
    }

    #[tokio::test]
    async fn get_room_set_temp_reads_from_modbus() {
        let (_dir, store) = tmp_store();
        let mut reads = HashMap::new();
        // factor 0.1: raw 221 -> 22.1
        reads.insert(HEATSYSTEM_ROOM_SETTEMP.id, 221_u16);
        let result = get_room_set_temp(State(fake_state(store, reads)))
            .await
            .unwrap();
        assert!(
            result.contains("\"room_temperature_setpoint\":"),
            "got: {result}"
        );
        assert!(result.contains("22.1"), "got: {result}");
    }

    #[tokio::test]
    async fn get_outdoor_temp_reads_from_modbus_on_cache_miss() {
        let (_dir, store) = tmp_store();
        let mut reads = HashMap::new();
        // factor 0.1, signed: raw 0xFFE0 (65504) -> -3.2
        reads.insert(CTC_OUTDOOR_TEMP.id, (-32_i16).cast_unsigned());
        let result = get_outdoor_temp(State(fake_state(store, reads)))
            .await
            .unwrap();
        assert!(result.contains("\"outdoor_temperature\":"), "got: {result}");
        assert!(result.contains("-3.2"), "got: {result}");
    }

    #[tokio::test]
    async fn get_flow_temp_reads_from_modbus_on_cache_miss() {
        let (_dir, store) = tmp_store();
        let mut reads = HashMap::new();
        reads.insert(HEATSYSTEM_FLOW_TEMP.id, 350_u16); // 35.0
        let result = get_flow_temp(State(fake_state(store, reads)))
            .await
            .unwrap();
        assert!(
            result.contains("\"flow_outlet_temperature\":"),
            "got: {result}"
        );
        assert!(result.contains("35"), "got: {result}");
    }

    #[tokio::test]
    async fn get_return_temp_reads_from_modbus_on_cache_miss() {
        let (_dir, store) = tmp_store();
        let mut reads = HashMap::new();
        reads.insert(CTC_RETURN_TEMP.id, 305_u16); // 30.5
        let result = get_return_temp(State(fake_state(store, reads)))
            .await
            .unwrap();
        assert!(
            result.contains("\"flow_return_temperature\":"),
            "got: {result}"
        );
        assert!(result.contains("30.5"), "got: {result}");
    }

    // --- Set setpoint: validation + write success ---

    #[tokio::test]
    async fn set_room_set_temp_writes_valid_value() {
        let (_dir, store) = tmp_store();
        // Write ops echo the value; 22.0 is within [10, 30].
        let result = set_room_set_temp(
            State(fake_state(store, HashMap::new())),
            Query(RoomSetPoint { value: 22.0 }),
        )
        .await
        .unwrap();
        assert!(
            result.contains("\"room_temperature_setpoint\":"),
            "got: {result}"
        );
        assert!(result.contains("22"), "got: {result}");
    }

    #[tokio::test]
    async fn set_room_set_temp_rejects_below_min() {
        let (_dir, store) = tmp_store();
        let err = set_room_set_temp(
            State(fake_state(store, HashMap::new())),
            Query(RoomSetPoint { value: 5.0 }),
        )
        .await
        .expect_err("below min should be rejected");
        assert!(matches!(err, ApiError::BadRequest));
    }

    #[tokio::test]
    async fn set_room_set_temp_rejects_above_max() {
        let (_dir, store) = tmp_store();
        let err = set_room_set_temp(
            State(fake_state(store, HashMap::new())),
            Query(RoomSetPoint { value: 99.0 }),
        )
        .await
        .expect_err("above max should be rejected");
        assert!(matches!(err, ApiError::BadRequest));
    }

    // --- Error paths: closed channel -> ServiceUnavailable ---

    #[tokio::test]
    async fn get_room_set_temp_service_unavailable_on_closed_channel() {
        let (_dir, store) = tmp_store();
        let err = get_room_set_temp(State(closed_state(store)))
            .await
            .expect_err("closed channel should fail");
        assert!(matches!(err, ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn get_room_temp_service_unavailable_on_closed_channel() {
        let (_dir, store) = tmp_store();
        // Empty store -> cache miss -> tries Modbus -> send fails.
        let err = get_room_temp(State(closed_state(store)))
            .await
            .expect_err("closed channel should fail");
        assert!(matches!(err, ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn set_room_set_temp_service_unavailable_on_closed_channel() {
        let (_dir, store) = tmp_store();
        let err = set_room_set_temp(
            State(closed_state(store)),
            Query(RoomSetPoint { value: 22.0 }),
        )
        .await
        .expect_err("closed channel should fail");
        assert!(matches!(err, ApiError::ServiceUnavailable));
    }
}
