use std::time::Duration;

use crate::config::TemperatureValidationConfig;
use crate::error::ApiError;
use crate::modbus::bms_parameters::{
    CTC_OUTDOOR_TEMP, CTC_RETURN_TEMP, CTC_ROOM_TEMP, HEATSYSTEM_FLOW_TEMP, HEATSYSTEM_ROOM_SETTEMP,
};
use crate::modbus::{ModbusSender, read_parameter, write_parameter};
use axum::{
    Router,
    extract::{Query, State},
    routing::{get, post},
};
use serde::Deserialize;

pub fn routes(
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
        .with_state((sender, validation_config, request_timeout_secs))
}

// read the actual room temperature
async fn get_room_temp(
    State((tx, _, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
) -> Result<String, ApiError> {
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
async fn get_room_set_temp(
    State((tx, _, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
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
    State((tx, config, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
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
    State((tx, _, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
) -> Result<String, ApiError> {
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
    State((tx, _, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
) -> Result<String, ApiError> {
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
    State((tx, _, timeout_secs)): State<(ModbusSender, TemperatureValidationConfig, u64)>,
) -> Result<String, ApiError> {
    read_parameter(
        &tx,
        CTC_RETURN_TEMP,
        "flow_return_temperature",
        "get_return_temp",
        Duration::from_secs(timeout_secs),
    )
    .await
}
