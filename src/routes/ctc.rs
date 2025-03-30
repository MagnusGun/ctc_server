use std::sync::Arc;
use ctc_server::modbus::bms_parameters::{CTC_VACCATION_DAYS, HEATPUMP_BLOCKED, HEATSYSTEM_HEATING_MODE, HEATSYSTEM_ROOM_SETTEMP};
use axum::{extract::{Query, State}, http::StatusCode, routing::{get, post}, Router};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_modbus::client::Context;
use ctc_server::modbus::{ModbusParameter, Access};

pub fn routes(state: &Arc<Mutex<Context>>) -> Router<Arc<Mutex<Context>>> {
    Router::new()
        .route("/api/v1/ctc/", get(get_ctc_data))
        .route("/api/v1/ctc/powersave", post(set_power_save))
        .route("/api/v1/ctc/powersave", get(get_power_save))
        .with_state(Arc::clone(&state))
}

#[derive(Debug, Deserialize, Clone, Copy)]
struct CtcParams {
    addr: u16,
    factor: f32,
    signed: bool,
}
impl CtcParams {
    fn new_modbus_parameter(self) -> ModbusParameter {
        ModbusParameter {
            id: self.addr,
            signed: self.signed,
            access: Access::R,
            reg_max: 0,
            reg_min: 0,
            reg_step: 0,
            visible: 0,
            bit: 0,
            factor: self.factor,
        }
    }
    
}

async fn get_ctc_data(State(state): State<Arc<Mutex<Context>>>, Query(params): Query<CtcParams>) -> Result<String, (StatusCode, String)>{
    let query = params.new_modbus_parameter();
    println!("get_ctc_data {query:?}");
    
    let ctx = state.lock().await;
    let rsp = query.read(ctx).await.unwrap();
    Ok(format!("{{\"ctc_data\": {rsp}}}\n"))
}

#[derive(Debug, Deserialize)]
struct PowerSave {
    active: bool,
}

async fn set_power_save(State(state): State<Arc<Mutex<Context>>>, Query(params): Query<PowerSave>) -> Result<String, (StatusCode, String)>{
    println!("set_power_save: {params:?}");
    match params.active {
        true => {
            // set the room temperature to 15 degrees
            {
                let ctx = state.lock().await;
                HEATSYSTEM_ROOM_SETTEMP.write(ctx, 15_f32).await.unwrap();
            }

            // // set the heating mode to 2 (off)
            // {
            //     let ctx = state.lock().await;
            //     HEATSYSTEM_HEATING_MODE.write(ctx, 2_f32).await.unwrap();
            // }

            // set the heatpump blocked to 0 (blocked) _deprecated
            // set vacation days to 2
            {
                let ctx = state.lock().await;
                CTC_VACCATION_DAYS.write(ctx, 2_f32).await.unwrap();
            }
        },
        false => {
            // set the room temperature to 21.5 degrees
            {
                let ctx = state.lock().await;
                HEATSYSTEM_ROOM_SETTEMP.write(ctx, 21.5_f32).await.unwrap();
            }

            // // set the heating mode to 0 (auto)
            // {
            //     let ctx = state.lock().await;
            //     HEATSYSTEM_HEATING_MODE.write(ctx, 0_f32).await.unwrap();
            // }

            // set the heatpump blocked to 1 (unblocked) _deprecated
            // set vacation days to 0
            {
                let ctx = state.lock().await;
                CTC_VACCATION_DAYS.write(ctx, 0_f32).await.unwrap();
            }
        }
    }

    let pow_state = params.active;
    Ok(format!("{{\"powersave\": {pow_state}}}\n"))
}

async fn get_power_save(State(state): State<Arc<Mutex<Context>>>) -> Result<String, (StatusCode, String)>{
    println!("get_power_save");
    let room_setpoint;
    let vaccation_days;
    // let heatpump_blocked;

    {
        let ctx = state.lock().await;
        room_setpoint = HEATSYSTEM_ROOM_SETTEMP.read(ctx).await.unwrap();
    }
    {
        let ctx = state.lock().await;
        vaccation_days = CTC_VACCATION_DAYS.read(ctx).await.unwrap();
    }
    // {   let ctx = state.lock().await;
    //     heating_mode = HEATSYSTEM_HEATING_MODE.read(ctx).await.unwrap();
    // }
    // {
    //     let ctx = state.lock().await;
    //     heatpump_blocked = HEATPUMP_BLOCKED.read(ctx).await.unwrap();
    // }

    // let heatpump_blocked = match heatpump_blocked {
    //     0.0 => "Blocked",
    //     1.0 => "Unblocked",
    //     _ => "Unknown",
    // };

    // let heating_mode = match heating_mode {
    //     0.0 => "Auto",
    //     1.0 => "Manual",
    //     2.0 => "Off",
    //     _ => "Unknown",
    // };

    Ok(format!("{{\"room_temp_setpoint\": {room_setpoint}, \"vaccation_days\": {vaccation_days}}}\n"))
}