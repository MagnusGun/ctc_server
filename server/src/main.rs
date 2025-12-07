mod config;
mod error;
mod gpio;
mod modbus;
mod routes;

use crate::config::Config;
use crate::error::ModbusError;
use crate::gpio::GpioController;
use crate::modbus::{CtcActorBuilder, ParameterOperation, SmartGridMode};
use axum::Router;
use std::{env, time::Duration};
use tracing::{debug, error, info};
// const SCALE_BASE: u16 = 10;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = Config::load(None)?;
    info!("Configuration loaded successfully");
    debug!(
        "Server config: {}:{}",
        config.server.host, config.server.port
    );

    // Get serial port from CLI args or use config default
    let mut args = env::args();
    let tty_path = args
        .nth(1)
        .unwrap_or_else(|| config.serial.default_port.clone());
    info!("Using serial port: {}", tty_path);

    debug!(
        "Available serial ports: {:?}",
        tokio_serial::available_ports()?
    );

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

    let (tx, rx) = tokio::sync::mpsc::channel::<(
        ParameterOperation,
        tokio::sync::oneshot::Sender<Result<f32, ModbusError>>,
    )>(config.modbus.channel_buffer_size);

    let mut ctc_actor = CtcActorBuilder::new(tty_path)
        .baud_rate(config.serial.baud_rate)
        .data_bits(config.serial.get_data_bits())
        .parity(config.serial.get_parity())
        .stop_bits(config.serial.get_stop_bits())
        .flow_control(config.serial.get_flow_control())
        .timeout(Duration::from_secs(config.serial.timeout_secs))
        .slave_id(config.modbus.slave_id)
        .operation_timeout(Duration::from_secs(config.modbus.operation_timeout_secs))
        .max_retries(config.modbus.max_retries)
        .initial_retry_delay(Duration::from_millis(config.modbus.initial_retry_delay_ms))
        .backoff_multiplier(config.modbus.backoff_multiplier)
        .max_consecutive_failures(config.modbus.max_consecutive_failures)
        .build(rx)?;

    tokio::spawn(async move {
        ctc_actor.run().await;
    });

    // Create GPIO controller if enabled (required for SmartGrid control)
    let gpio_controller = if config.gpio.enabled {
        info!(
            "GPIO control enabled: K24=GPIO{}, K25=GPIO{}, active_low={}",
            config.gpio.gpio_k24, config.gpio.gpio_k25, config.gpio.active_low
        );
        let controller = GpioController::new(
            config.gpio.gpio_k24,
            config.gpio.gpio_k25,
            config.gpio.active_low,
        );
        // Initialize to Normal mode on startup
        if let Err(e) = controller.set_mode(SmartGridMode::Normal) {
            error!("Failed to initialize GPIO to Normal mode: {}", e);
        } else {
            info!("GPIO initialized to Normal mode");
        }
        Some(controller)
    } else {
        debug!("GPIO control disabled - SmartGrid endpoints will return ServiceUnavailable");
        None
    };

    let app = Router::new()
        // .route("/ctc", get(ctx_handler))
        .merge(routes::temperatures::routes(
            tx.clone(),
            config.temperature_validation.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::ctc::routes(
            tx.clone(),
            config.modbus.request_timeout_secs,
            gpio_controller.clone(),
        ))
        .merge(routes::smartgrid::routes(
            gpio_controller,
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::visibility::routes(
            tx,
            config.modbus.request_timeout_secs,
        ));

    // Set up the server to listen
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);
    info!("Starting server on {}", bind_addr);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
