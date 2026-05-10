mod config;
mod energy;
mod error;
mod heatpump;
mod messages;
mod modbus;
mod routes;
mod smartgrid;

use crate::config::Config;
use crate::energy::{GridState, PriceState};
use crate::modbus::CtcActorBuilder;
use crate::modbus::actor::ModbusRequest;
use crate::smartgrid::GpioController;
use crate::smartgrid::SmartGridMode;
use axum::{Router, response::Redirect, routing::get};
use std::{env, path::PathBuf, time::Duration};
use tower_http::services::ServeDir;
use tracing::{debug, error, info};

/// Returns the path to the static files directory
fn static_dir() -> PathBuf {
    // Try relative path first (for development)
    let relative = PathBuf::from("static");
    if relative.exists() {
        return relative;
    }
    // Try from executable location (for deployment)
    if let Ok(exe_path) = env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        let from_exe = exe_dir.join("static");
        if from_exe.exists() {
            return from_exe;
        }
    }
    // Fallback to relative path
    relative
}

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::too_many_lines)]
// Server initialization is sequential and logically coherent
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Install rustls crypto provider (required for rustls 0.23+)
    // aws-lc-rs is set as the only crypto feature in Cargo.toml
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

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

    let (tx, rx) = tokio::sync::mpsc::channel::<ModbusRequest>(config.modbus.channel_buffer_size);

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

        // Parse CTC_POWERSAVE environment variable for initial mode
        let initial_powersave = env::var("CTC_POWERSAVE")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .unwrap_or(false);

        let initial_mode = if initial_powersave {
            SmartGridMode::Blocking
        } else {
            SmartGridMode::Normal
        };

        if let Err(e) = controller.set_mode(initial_mode) {
            error!("Failed to initialize GPIO to {} mode: {}", initial_mode, e);
        } else {
            info!(
                "GPIO initialized to {} mode (CTC_POWERSAVE={})",
                initial_mode, initial_powersave
            );
        }
        Some(controller)
    } else {
        debug!("GPIO control disabled - SmartGrid endpoints will return ServiceUnavailable");
        None
    };

    // Create grid state for peak tracking
    let grid_state = GridState::new();

    // Create heat pump stats tracker.
    // When `persist_path` is configured (and non-empty), accumulators and
    // history survive restarts; otherwise the tracker is in-memory only.
    let heatpump_stats = match config.heatpump_stats.persist_path.as_deref() {
        Some(p) if !p.is_empty() => {
            info!("Heat pump stats persistence enabled at {p}");
            heatpump::HeatPumpStats::new_with_persistence(p)
        }
        _ => heatpump::HeatPumpStats::new(),
    };

    // Start heat pump stats polling if enabled
    if config.heatpump_stats.enabled {
        info!(
            "Heat pump statistics enabled: poll_interval={}s",
            config.heatpump_stats.poll_interval_secs
        );

        let stats_clone = heatpump_stats.clone();
        let modbus_tx_clone = tx.clone();
        let poll_interval = config.heatpump_stats.poll_interval_secs;
        let request_timeout = config.modbus.request_timeout_secs;

        tokio::spawn(async move {
            heatpump::poller::run_poll_loop(
                modbus_tx_clone,
                stats_clone,
                poll_interval,
                request_timeout,
            )
            .await;
        });
    } else {
        debug!("Heat pump statistics tracking disabled");
    }

    // Start Tibber WebSocket if configured
    let tibber_token = if config.tibber.enabled {
        if let Some(ref token) = config.tibber.api.token {
            info!("Tibber integration enabled");

            // Spawn WebSocket background task for real-time consumption data
            let api_token = token.clone();
            let grid_state_clone = grid_state.clone();
            tokio::spawn(async move {
                energy::tibber::run_websocket_loop(api_token, grid_state_clone).await;
            });
            Some(token.clone())
        } else {
            info!("Tibber enabled but no API token configured");
            None
        }
    } else {
        debug!("Tibber integration disabled");
        None
    };

    // Create price state for electricity price tracking
    let price_state = PriceState::new(config.price.zone.clone());

    // Start price fetch background task if enabled
    if config.price.enabled {
        info!(
            "Price tracking enabled: zone={}, interval={}min",
            config.price.zone, config.price.fetch_interval_mins
        );

        let price_state_clone = price_state.clone();
        let tibber_token_clone = tibber_token.clone();
        let price_zone = config.price.zone.clone();
        let fetch_interval = config.price.fetch_interval_mins;

        tokio::spawn(async move {
            run_price_fetch_loop(
                price_state_clone,
                tibber_token_clone,
                price_zone,
                fetch_interval,
            )
            .await;
        });
    } else {
        debug!("Price tracking disabled");
    }

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
            tx.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::alarms::routes(
            tx,
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::grid::routes(
            grid_state,
            price_state,
            tibber_token.is_some(),
        ))
        .merge(routes::heatpump_stats::routes(heatpump_stats.clone()))
        // Static file serving for web dashboard
        .nest_service("/static", ServeDir::new(static_dir()))
        .route(
            "/",
            get(|| async { Redirect::permanent("/static/index.html") }),
        );

    // Set up the server to listen
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);
    info!("Starting server on {}", bind_addr);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    // Graceful shutdown: on Ctrl-C, persist heatpump stats one last time
    // before exiting so the final accumulator state survives the restart.
    let stats_for_shutdown = heatpump_stats.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if let Err(e) = tokio::signal::ctrl_c().await {
                error!("Failed to install Ctrl-C handler: {e}");
                return;
            }
            info!("Shutdown signal received â persisting heatpump stats");
            if let Err(e) = stats_for_shutdown.save_to_disk() {
                error!("Failed to save heatpump stats on shutdown: {e}");
            }
        })
        .await?;

    Ok(())
}

/// Background task for fetching electricity prices
///
/// Fetches prices from both elprisetjustnu.se (spot prices) and Tibber (if configured).
/// Runs at the specified interval, aligned to 15-minute boundaries.
async fn run_price_fetch_loop(
    price_state: PriceState,
    tibber_token: Option<String>,
    price_zone: String,
    fetch_interval_mins: u64,
) {
    use crate::energy::{elpris::ElprisClient, tibber::fetch_prices as fetch_tibber_prices};
    use tokio::time::{Duration, interval};

    let elpris_client = ElprisClient::new(&price_zone);
    let mut ticker = interval(Duration::from_secs(fetch_interval_mins * 60));

    info!("Price fetch loop started");

    loop {
        ticker.tick().await;

        info!("Fetching electricity prices...");

        // Fetch from elprisetjustnu.se (always)
        let elpris_today = match elpris_client.get_today_prices().await {
            Ok(prices) => {
                info!("Fetched {} spot prices for today", prices.len());
                prices
            }
            Err(e) => {
                error!("Failed to fetch today's spot prices: {}", e);
                continue;
            }
        };

        let elpris_tomorrow = elpris_client.try_get_tomorrow_prices().await;
        if let Some(ref prices) = elpris_tomorrow {
            info!("Fetched {} spot prices for tomorrow", prices.len());
        }

        // Fetch from Tibber (if token configured)
        let tibber_data = if let Some(ref token) = tibber_token {
            fetch_tibber_prices(token).await
        } else {
            None
        };

        // Merge prices from both sources
        let today_prices = merge_prices(&elpris_today, tibber_data.as_ref().map(|d| &d.today));
        let tomorrow_prices = elpris_tomorrow.as_ref().map_or_else(Vec::new, |elpris| {
            merge_prices(elpris, tibber_data.as_ref().map(|d| &d.tomorrow))
        });

        // Calculate price levels if Tibber data is not available
        let today_prices = calculate_levels_if_missing(today_prices);
        let tomorrow_prices = calculate_levels_if_missing(tomorrow_prices);

        // Update state
        price_state.update_prices(today_prices, tomorrow_prices);

        info!("Price state updated successfully");
    }
}

/// Merge elpris spot prices with Tibber prices
fn merge_prices(
    elpris: &[crate::energy::elpris::ElprisEntry],
    tibber: Option<&Vec<crate::energy::tibber::TibberPrice>>,
) -> Vec<crate::energy::price::PricePoint> {
    use crate::energy::price::{PriceLevel, PricePoint};

    elpris
        .iter()
        .map(|e| {
            let mut point = PricePoint::from_spot(
                e.time_start.clone(),
                e.time_end.clone(),
                e.sek_per_kwh,
                e.eur_per_kwh,
                e.exchange_rate,
            );

            // Try to find matching Tibber price by start time
            if let Some(tibber_prices) = tibber
                && let Some(tibber) = tibber_prices.iter().find(|t| t.starts_at == e.time_start)
            {
                let level = tibber
                    .level
                    .as_ref()
                    .and_then(|l| PriceLevel::from_tibber_str(l));
                point = point.with_tibber(tibber.total, tibber.energy, tibber.tax, level);
            }

            point
        })
        .collect()
}

/// Calculate price levels based on percentile if Tibber levels are missing
#[allow(clippy::cast_precision_loss)]
fn calculate_levels_if_missing(
    mut prices: Vec<crate::energy::price::PricePoint>,
) -> Vec<crate::energy::price::PricePoint> {
    use crate::energy::price::PriceLevel;

    // Check if any prices need level calculation
    let needs_levels = prices.iter().any(|p| p.level.is_none());
    if !needs_levels || prices.is_empty() {
        return prices;
    }

    // Sort prices by spot price to calculate percentiles
    let mut sorted_prices: Vec<f64> = prices.iter().map(|p| p.spot_sek).collect();
    sorted_prices.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Calculate level for each price based on percentile
    for price in &mut prices {
        if price.level.is_none() {
            let position = sorted_prices
                .iter()
                .position(|&p| (p - price.spot_sek).abs() < f64::EPSILON)
                .unwrap_or(0);
            let percentile = position as f64 / sorted_prices.len() as f64;
            price.level = Some(PriceLevel::from_percentile(percentile));
        }
    }

    prices
}
