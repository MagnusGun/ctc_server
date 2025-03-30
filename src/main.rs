mod routes;

use std::{env, str, sync::Arc, time::Duration};
use axum::Router;
use tokio::sync::Mutex;
use tokio_modbus::{client::rtu, Slave};
use tokio_serial::{Parity, SerialPortBuilderExt, StopBits};

const DEFAULT_TTY: &str = "/dev/ttyAMA4";
// const SCALE_BASE: u16 = 10;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let tty_path = args.nth(1).unwrap_or_else(|| DEFAULT_TTY.into());
    tracing_subscriber::fmt::init();

    println!("Available serial ports: {:?}",tokio_serial::available_ports()?);

    let port = tokio_serial::new(tty_path, 9600)
        .baud_rate(9600)
        .data_bits(tokio_serial::DataBits::Eight)
        .parity(Parity::Even)
        .stop_bits(StopBits::One)
        .flow_control(tokio_serial::FlowControl::Hardware)
        .timeout(Duration::from_secs(1))
        .open_native_async()?;

    println!("Port: {port:?}");

    let ctx = rtu::attach_slave(port, Slave(1));
    let shared_ctx = Arc::new(Mutex::new(ctx));

    let app = Router::new()
        // .route("/ctc", get(ctx_handler))
        .merge(routes::temperatures::routes(&shared_ctx))
        .merge(routes::ctc::routes(&shared_ctx))
        .with_state(Arc::clone(&shared_ctx));    

    // println!("reading target temperature");
    // let rsp = ctx.read_holding_registers(61509, 1).await??;
    // println!("Response: {:?}", rsp);

    // println!("Setting target temperature to 300");
    // let tmp:u16 = 300;
    // let rsp = ctx.write_multiple_registers(61509, &[tmp]).await??;
    // println!("Response: {:?}", rsp);

    // println!("reading current target temperature");
    // let rsp = ctx.read_holding_registers(61509, 1).await??;
    // println!("Response: {:?}", rsp);

    // println!("Setting target temperature to 205");
    // let tmp:u16 = 205;
    // let rsp = ctx.write_multiple_registers(61509, &[tmp]).await??;
    // println!("Response: {:?}", rsp);

    // println!("reading current target temperature");
    // let rsp = ctx.read_holding_registers(61509, 1).await??;
    // println!("Response: {:?}", rsp);

    // println!("Disconnecting from slave");
    // ctx.disconnect().await?;

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();


    Ok(())
}

// async fn ctx_handler(State(state): State<Arc<Mutex<Context>>>, Query(params): Query<CtcParams>) -> Result<String, (StatusCode, String)> {

//     println!("Received parameters: {params:?}");
//     let scale = SCALE_BASE.pow(params.scale.into());

//     let mut guard = state.lock().await;
//     match params.cmd {
//         Command::Read => guard
//             .read_holding_registers(params.addr, params.cnt)
//             .await
//             .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
//             .and_then(|rsp| {
//                 rsp.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
//                 .and_then(|res| {
//                     res.first()
//                         .map(|value|{
//                             println!("value: {value}");
//                             println!("scale: {scale}");
//                             let scaled_value: f32 = f32::from(*value)/f32::from(scale);
//                             format!("{scaled_value}")
//                         })
//                         .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "Empty response\n".into()))
//                 })
//         }),
//         Command::Write => {
//             let data = params.data.ok_or((StatusCode::BAD_REQUEST, "Missing data field\n".to_string()))?;
//             let scaled_data = (data as f32 * f32::from(scale)).round() as u16;
//             println!("Writing scaled data: {scaled_data}");
        
//             guard.write_multiple_registers(params.addr, &[scaled_data])
//                 .await
//                 .map_err(|e|(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
//                 .map(|_| "Write successful\n".to_string())
//         }
//     }
// }

// #[derive(Debug, Deserialize)]
// #[serde(rename_all = "lowercase")]
// enum Command {
//     Read,
//     Write,
// }

// fn default_cnt() -> u16 {
//     1
// }

// fn default_scale() -> u16 {
//     1
// }

// #[derive(Debug, Deserialize)]
// struct CtcParams {
//     cmd: Command,
//     addr: u16,
//     #[serde(default = "default_cnt")]
//     cnt: u16,
//     #[serde(default = "default_scale")]
//     scale: u16,
//     // `data` remains optional, for example:
//     data: Option<f32>,
// }