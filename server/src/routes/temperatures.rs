// use crate::modbus::bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, CTC_ROOM_TEMP, CTC_OUTDOOR_TEMP, HEATSYSTEM_FLOW_TEMP, CTC_RETURN_TEMP};
use crate::modbus::bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, CTC_ROOM_TEMP, CTC_OUTDOOR_TEMP, HEATSYSTEM_FLOW_TEMP, CTC_RETURN_TEMP};
use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;
use tracing::{debug, error};

use crate::routes::ctc_actor::{ModbusSender, ParameterOperation};

pub fn routes(sender: tokio::sync::mpsc::Sender<(ParameterOperation, tokio::sync::oneshot::Sender<Result<f32,String>>)>) 
    -> Router {
    Router::new()
        .route("/api/v1/temperature/room", get(get_room_temp))
        .route("/api/v1/temperature/room/setpoint", get(get_room_set_temp))
        .route("/api/v1/temperature/room/setpoint/", post(set_room_set_temp))
        .route("/api/v1/temperature/outdoor", get(get_outdoor_temp))
        .route("/api/v1/temperature/flow", get(get_flow_temp))
        .route("/api/v1/temperature/flow/return", get(get_return_temp))
        .with_state(sender)
}

// read the actual room temperature
async fn get_room_temp(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)>  {
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(CTC_ROOM_TEMP), response_tx)).await.unwrap();
    
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_room_temp: {rsp}");
            Ok(format!("{{\"room_temperature\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("Error reading room temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }   
}

// read the room temperature setpoint
async fn get_room_set_temp(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(HEATSYSTEM_ROOM_SETTEMP), response_tx)).await.unwrap();
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_room_set_temp: {rsp}");
            Ok(format!("{{\"room_temperarure_setpoint\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("Error reading room set temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}

#[derive(Debug, Deserialize)]
struct RoomSetPoint {
    value: f32,
}

// sets the room temperature setpoint
async fn set_room_set_temp(State(tx): State<ModbusSender>, Query(param): Query<RoomSetPoint>) -> Result<String, (StatusCode, String)>{
    if param.value < 5.0 || param.value > 30.0 {
        return Err((StatusCode::BAD_REQUEST, "Invalid temperature value".to_string()));
    }

    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Write(HEATSYSTEM_ROOM_SETTEMP, param.value), response_tx)).await.unwrap();
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(_)) => {
            debug!("set_room_set_temp: {}", param.value);
            Ok(format!("{{\"room_temperarure_setpoint\": {}}}\n", param.value))
        },
        Ok(Err(e)) => {
            error!("Error setting room set temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}

// read the outdoor temperature              
async fn get_outdoor_temp(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(CTC_OUTDOOR_TEMP), response_tx)).await.unwrap();
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_outdoor_temp: {rsp}");
            Ok(format!("{{\"outdoor_temperature\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("Error reading outdoor temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}

// read heatpump outgoing flow temperature
async fn get_flow_temp(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(HEATSYSTEM_FLOW_TEMP), response_tx)).await.unwrap();
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_flow_temp: {rsp}");
            Ok(format!("{{\"flow_outlet_temperature\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("Error reading flow temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}

// read heatpump incoming flow temperature
async fn get_return_temp(State(tx): State<ModbusSender>) -> Result<String, (StatusCode, String)> {
    // Create a oneshot channel for THIS REQUEST
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    // Send the operation and response channel to the actor
    tx.send((ParameterOperation::Read(CTC_RETURN_TEMP), response_tx)).await.unwrap();
    // Wait for response on THIS request's channel
    match response_rx.await {
        Ok(Ok(rsp)) => {
            debug!("get_return_temp: {rsp}");
            Ok(format!("{{\"flow_return_temperature\": {rsp}}}\n"))
        },
        Ok(Err(e)) => {
            error!("Error reading return temperature: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, e))
        },
        Err(e) => {
            error!("Failed to receive response: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to receive response".to_string()))
        }
    }
}