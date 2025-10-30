use crate::config::TemperatureValidationConfig;
use crate::modbus::bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, CTC_ROOM_TEMP, CTC_OUTDOOR_TEMP, HEATSYSTEM_FLOW_TEMP, CTC_RETURN_TEMP};
use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;

use crate::helpers::{read_parameter, write_parameter};
use crate::routes::ctc_actor::{ModbusSender, ParameterOperation};

pub fn routes(
    sender: tokio::sync::mpsc::Sender<(ParameterOperation, tokio::sync::oneshot::Sender<Result<f32,String>>)>,
    validation_config: TemperatureValidationConfig,
) -> Router {
    Router::new()
        .route("/api/v1/temperature/room", get(get_room_temp))
        .route("/api/v1/temperature/room/setpoint", get(get_room_set_temp))
        .route("/api/v1/temperature/room/setpoint/", post(set_room_set_temp))
        .route("/api/v1/temperature/outdoor", get(get_outdoor_temp))
        .route("/api/v1/temperature/flow", get(get_flow_temp))
        .route("/api/v1/temperature/flow/return", get(get_return_temp))
        .with_state((sender, validation_config))
}

// read the actual room temperature
async fn get_room_temp(State((tx, _)): State<(ModbusSender, TemperatureValidationConfig)>) -> Result<String, (StatusCode, String)>  {
    read_parameter(&tx, CTC_ROOM_TEMP, "room_temperature", "get_room_temp").await
}

// read the room temperature setpoint
async fn get_room_set_temp(State((tx, _)): State<(ModbusSender, TemperatureValidationConfig)>) -> Result<String, (StatusCode, String)> {
    read_parameter(&tx, HEATSYSTEM_ROOM_SETTEMP, "room_temperature_setpoint", "get_room_set_temp").await
}

#[derive(Debug, Deserialize)]
struct RoomSetPoint {
    value: f32,
}

// sets the room temperature setpoint
async fn set_room_set_temp(State((tx, config)): State<(ModbusSender, TemperatureValidationConfig)>, Query(param): Query<RoomSetPoint>) -> Result<String, (StatusCode, String)>{
    if param.value < config.min || param.value > config.max {
        return Err((StatusCode::BAD_REQUEST, format!("Invalid temperature value. Must be between {} and {}", config.min, config.max)));
    }

    write_parameter(&tx, HEATSYSTEM_ROOM_SETTEMP, param.value, "room_temperature_setpoint", "set_room_set_temp").await
}

// read the outdoor temperature
async fn get_outdoor_temp(State((tx, _)): State<(ModbusSender, TemperatureValidationConfig)>) -> Result<String, (StatusCode, String)> {
    read_parameter(&tx, CTC_OUTDOOR_TEMP, "outdoor_temperature", "get_outdoor_temp").await
}

// read heatpump outgoing flow temperature
async fn get_flow_temp(State((tx, _)): State<(ModbusSender, TemperatureValidationConfig)>) -> Result<String, (StatusCode, String)> {
    read_parameter(&tx, HEATSYSTEM_FLOW_TEMP, "flow_outlet_temperature", "get_flow_temp").await
}

// read heatpump incoming flow temperature
async fn get_return_temp(State((tx, _)): State<(ModbusSender, TemperatureValidationConfig)>) -> Result<String, (StatusCode, String)> {
    read_parameter(&tx, CTC_RETURN_TEMP, "flow_return_temperature", "get_return_temp").await
}