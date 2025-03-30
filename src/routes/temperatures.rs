

use std::sync::Arc;
use ctc_server::modbus::bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, CTC_ROOM_TEMP, CTC_OUTDOOR_TEMP, HEATSYSTEM_FLOW_TEMP, CTC_RETURN_TEMP};


use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_modbus::client::Context;

pub fn routes(state: &Arc<Mutex<Context>>) -> Router<Arc<Mutex<Context>>> {
    Router::new()
        .route("/api/v1/temperature/room", get(get_room_temp))
        .route("/api/v1/temperature/room/setpoint", get(get_room_set_temp))
        .route("/api/v1/temperature/room/setpoint/", post(set_room_set_temp))
        .route("/api/v1/temperature/outdoor", get(get_outdoor_temp))
        .route("/api/v1/temperature/flow", get(get_flow_temp))
        .route("/api/v1/temperature/flow/return", get(get_return_temp))
        .with_state(Arc::clone(&state))
}

// read the actual room temperature
async fn get_room_temp(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)>  {
    println!("get_room_temp");
    let ctx = state.lock().await;
    let rsp = CTC_ROOM_TEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"room_temperature\": {rsp}}}\n"))
}

// read the room temperature setpoint
async fn get_room_set_temp(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)> {
    println!("get_room_set_temp");
    let ctx = state.lock().await;
    let rsp = HEATSYSTEM_ROOM_SETTEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"room_temperarure_setpoint\": {rsp}}}\n"))
}

#[derive(Debug, Deserialize)]
struct RoomSetPoint {
    value: f32,
}

// sets the room temperature setpoint
async fn set_room_set_temp(State(state): State<Arc<Mutex<Context>>>, Query(param): Query<RoomSetPoint>) -> Result<String, (StatusCode, String)>{
    if param.value < 5.0 || param.value > 30.0 {
        return Err((StatusCode::BAD_REQUEST, "Invalid temperature value".to_string()));
    }
    println!("set_room_set_temp: {}",param.value);
    {
        let ctx = state.lock().await;
        HEATSYSTEM_ROOM_SETTEMP.write(ctx, param.value).await.unwrap();
    }
    let ctx = state.lock().await;
    let rsp = HEATSYSTEM_ROOM_SETTEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"room_temperarure_setpoint\": {rsp}}}\n"))
}

// read the outdoor temperature              
async fn get_outdoor_temp(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)> {
    println!("get_outdoor_temp");
    let ctx = state.lock().await;
    let rsp = CTC_OUTDOOR_TEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"outdoor_temperature\": {rsp}}}\n"))
}

// read heatpump outgoing flow temperature
async fn get_flow_temp(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)> {
    println!("get_flow_temp");
    let ctx = state.lock().await;
    let rsp = HEATSYSTEM_FLOW_TEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"flow_oulet_temperature\": {rsp}}}\n"))
}

// read heatpump incoming flow temperature
async fn get_return_temp(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)> {
    println!("get_return_temp");
    let ctx = state.lock().await;
    let rsp = CTC_RETURN_TEMP.read(ctx).await.unwrap();
    Ok(format!("{{\"flow_return_temperature\": {rsp}}}\n"))
}