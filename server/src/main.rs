mod routes;
mod modbus;
mod helpers;

use std::{env, str, time::Duration};
use axum::Router;
use tokio_serial::{Parity, StopBits};
use tracing::debug;
use crate::routes::{ctc_actor::{CtcActorBuilder, ParameterOperation}};


const DEFAULT_TTY: &str = "/dev/ttyAMA4";
// const SCALE_BASE: u16 = 10;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let tty_path = args.nth(1).unwrap_or_else(|| DEFAULT_TTY.into());
    tracing_subscriber::fmt::init();
    debug!("Available serial ports: {:?}",tokio_serial::available_ports()?);


    // let port = tokio_serial::new(tty_path, 9600)
    //     .baud_rate(9600)
    //     .data_bits(tokio_serial::DataBits::Eight)
    //     .parity(Parity::Even)
    //     .stop_bits(StopBits::One)
    //     .flow_control(tokio_serial::FlowControl::Hardware)
    //     .timeout(Duration::from_secs(1))
    //     .open_native_async()?;
    // debug!("Port: {port:?}");

    // let ctx = rtu::attach_slave(port, Slave(1));
    // let shared_ctx = Arc::new(Mutex::new(ctx));

    let (tx, rx) = tokio::sync::mpsc::channel::<(ParameterOperation, tokio::sync::oneshot::Sender<Result<f32, String>>)>(24);
    
    let mut ctc_actor = CtcActorBuilder::new(tty_path)
        .baud_rate(9600)
        .data_bits(tokio_serial::DataBits::Eight)
        .parity(Parity::Even)
        .stop_bits(StopBits::One)
        .flow_control(tokio_serial::FlowControl::Hardware)
        .timeout(Duration::from_secs(1))
        .build(rx)?;

    tokio::spawn(async move {
        ctc_actor.run().await;
    }); 

    let app = Router::new()
        // .route("/ctc", get(ctx_handler))
        .merge(routes::temperatures::routes(tx.clone()))
        .merge(routes::ctc::routes(tx.clone()));

    // Set up the server to listen on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();


    Ok(())
}