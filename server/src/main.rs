mod config;
mod energy;
mod error;
mod heatpump;
mod messages;
mod modbus;
mod routes;
mod smartgrid;
mod storage;
mod supervisor;

use crate::config::Config;
use crate::energy::{GridState, PriceState};
use crate::modbus::actor::ModbusRequest;
use crate::modbus::{CtcActorBuilder, SupervisorStats};
use crate::smartgrid::SmartGridMode;
use crate::smartgrid::actor as smartgrid_actor;
use axum::{Router, response::Redirect, routing::get};
use std::{
    env,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tower_http::services::ServeDir;
use tracing::{debug, error, info, warn};

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

    // Parse the configured timezone once. `load()` validated the string, so
    // this never panics in practice.
    let tz = config.parsed_tz();
    info!("Local timezone: {}", config.tz);

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

    let (tx, rx) = tokio::sync::mpsc::channel::<ModbusRequest>(config.modbus.channel_buffer_size);

    // Process-lifetime supervisor counters. The actor's in-memory stats
    // (histograms, per-register counters, rate window) reset on every
    // respawn; these atomics survive because they live above the actor.
    let sup_stats = Arc::new(SupervisorStats::default());

    CtcActorBuilder::new(tty_path)
        .baud_rate(config.serial.baud_rate)
        .data_bits(config.serial.get_data_bits()?)
        .parity(config.serial.get_parity()?)
        .stop_bits(config.serial.get_stop_bits()?)
        .flow_control(config.serial.get_flow_control()?)
        .timeout(Duration::from_secs(config.serial.timeout_secs))
        .slave_id(config.modbus.slave_id)
        .operation_timeout(Duration::from_secs(config.modbus.operation_timeout_secs))
        .max_retries(config.modbus.max_retries)
        .initial_retry_delay(Duration::from_millis(config.modbus.initial_retry_delay_ms))
        .backoff_multiplier(config.modbus.backoff_multiplier)
        .max_consecutive_failures(config.modbus.max_consecutive_failures)
        .inter_request_gap(Duration::from_millis(config.modbus.inter_request_gap_ms))
        .sup_stats(Arc::clone(&sup_stats))
        .spawn_supervised(rx);

    // Open the sensor cache store. Path is configurable via
    // `[storage] db_path` (config file) or `CTC_DB_PATH` env var.
    let db_path = PathBuf::from(&config.storage.db_path);
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let store = storage::Store::open(&db_path)?;
    info!("Sensor cache store opened at {}", db_path.display());

    // Single cancellation token shared by every background loop. The graceful
    // shutdown closure cancels it before flushing so loops exit cleanly
    // instead of being dropped mid-await on a redb transaction or RTU write.
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut background_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // Spawn the sensor cache poller. Every dashboard status read serves from
    // the in-memory ring this fills; the Modbus actor is reserved for writes
    // and one-off reads outside the polled set.
    {
        let store_for_poller = store.clone();
        let modbus_tx = tx.clone();
        let request_timeout = config.modbus.request_timeout_secs;
        // 5 s matches the previous dashboard refresh cadence.
        let poll_interval_secs = 5;
        let cancel_for_poller = cancel.clone();
        background_tasks.push(supervisor::spawn_with_shutdown(
            "sensor-poller",
            cancel.clone(),
            async move {
                storage::poller::run_sensor_poll_loop(
                    modbus_tx,
                    store_for_poller,
                    poll_interval_secs,
                    request_timeout,
                    cancel_for_poller,
                )
                .await;
            },
        ));
    }

    // Hourly flush. Keeps write amplification trivial: ~24 commits/day plus
    // one on graceful shutdown. An initial flush ~30 s after startup persists
    // the first poll-cycle accumulator snapshot so `kill -9` inside the first
    // hour doesn't lose all of it.
    {
        let store_for_flush = store.clone();
        let cancel_for_flush = cancel.clone();
        background_tasks.push(supervisor::spawn_with_shutdown(
            "hourly-flush",
            cancel.clone(),
            async move {
                // 30 s is well past the 5 s sensor-cache and 10 s heatpump-stats
                // default poll ticks, so the ring + accumulators are populated.
                tokio::select! {
                    biased;
                    () = cancel_for_flush.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
                if let Err(e) = store_for_flush.flush() {
                    error!("First-boot store flush failed: {e}");
                }
                let mut tick = tokio::time::interval(Duration::from_secs(3600));
                tick.tick().await; // consume the immediate first tick
                loop {
                    tokio::select! {
                        biased;
                        () = cancel_for_flush.cancelled() => {
                            info!("Hourly flush task: shutdown signal received");
                            return;
                        }
                        _ = tick.tick() => {}
                    }
                    if let Err(e) = store_for_flush.flush() {
                        error!("Hourly store flush failed: {e}");
                    }
                }
            },
        ));
    }

    // Create grid state for peak tracking, keyed to the configured tz.
    let grid_state = GridState::with_tz(tz);

    // One-shot migration: if a legacy heatpump_stats JSON is present, fold
    // it into the redb store and rename the file `.migrated` so the next
    // boot skips this path.
    if let Some(p) = config
        .heatpump_stats
        .persist_path
        .as_deref()
        .filter(|p| !p.is_empty())
    {
        let json_path = std::path::Path::new(p);
        match store.migrate_from_legacy_json(json_path) {
            Ok(true) => info!(
                "Migrated legacy heatpump-stats JSON from {} into store",
                json_path.display()
            ),
            Ok(false) => {}
            Err(e) => error!(
                "Failed to migrate legacy heatpump-stats JSON from {}: {e}",
                json_path.display()
            ),
        }
    }

    // Create the heat pump stats tracker backed by the store. Accumulators
    // and cycle history persist across restarts via the store's hourly
    // flush + graceful-shutdown flush.
    let heatpump_stats = heatpump::HeatPumpStats::new_with_store_and_tz(store.clone(), tz);

    // Start heat pump stats polling if enabled. Reads HEATPUMP_STATUS /
    // CTC_OUTDOOR_TEMP from the sensor cache instead of issuing its own
    // Modbus calls, so the actor mutex is uncontended.
    if config.heatpump_stats.enabled {
        info!(
            "Heat pump statistics enabled: poll_interval={}s",
            config.heatpump_stats.poll_interval_secs
        );

        let stats_clone = heatpump_stats.clone();
        let store_for_stats = store.clone();
        let poll_interval = config.heatpump_stats.poll_interval_secs;
        let cancel_for_stats = cancel.clone();

        background_tasks.push(supervisor::spawn_with_shutdown(
            "heatpump-stats-poller",
            cancel.clone(),
            async move {
                heatpump::poller::run_poll_loop(
                    store_for_stats,
                    stats_clone,
                    poll_interval,
                    cancel_for_stats,
                )
                .await;
            },
        ));
    } else {
        debug!("Heat pump statistics tracking disabled");
    }

    // Step-response recorder. Watches Flow / Return samples from the sensor
    // cache; persists events via the redb store.
    {
        let store_for_step = store.clone();
        let cancel_for_step = cancel.clone();
        background_tasks.push(supervisor::spawn_with_shutdown(
            "step-detector",
            cancel.clone(),
            async move {
                heatpump::step_detector::run_recorder_loop(store_for_step, 5, cancel_for_step)
                    .await;
            },
        ));
    }

    // Start Tibber WebSocket if configured (consumption stream only)
    if config.tibber.enabled {
        if let Some(ref token) = config.tibber.api.token {
            info!("Tibber WS (consumption) enabled");

            // Spawn WebSocket background task for real-time consumption data
            let api_token = token.clone();
            let grid_state_clone = grid_state.clone();
            let cancel_for_tibber = cancel.clone();
            background_tasks.push(supervisor::spawn_with_shutdown(
                "tibber-websocket",
                cancel.clone(),
                async move {
                    energy::tibber::run_websocket_loop(
                        api_token,
                        grid_state_clone,
                        tz,
                        cancel_for_tibber,
                    )
                    .await;
                },
            ));
        } else {
            info!("Tibber enabled but no API token configured");
        }
    } else {
        debug!("Tibber integration disabled");
    }

    // Create price state for electricity price tracking
    let price_state = PriceState::new(config.price.zone.clone());

    // Spawn the SmartGrid actor (required for SmartGrid control). Owns the
    // GpioController and processes all set-mode / read-mode / scheduled-
    // resume commands serially. Routes get a cheap-clone SmartGridHandle.
    let smartgrid_handle = if config.gpio.enabled {
        info!(
            "GPIO control enabled: K24=GPIO{}, K25=GPIO{}, active_low={}",
            config.gpio.gpio_k24, config.gpio.gpio_k25, config.gpio.active_low
        );
        let initial_powersave = env::var("CTC_POWERSAVE")
            .ok()
            .and_then(|v| v.parse::<bool>().ok())
            .unwrap_or(false);
        let initial_mode = if initial_powersave {
            SmartGridMode::Blocking
        } else {
            SmartGridMode::Normal
        };

        match smartgrid_actor::spawn(
            config.gpio.gpio_k24,
            config.gpio.gpio_k25,
            config.gpio.active_low,
            initial_mode,
            price_state.clone(),
            config.smartgrid.clone(),
            cancel.clone(),
        ) {
            Ok((handle, join)) => {
                info!(
                    "SmartGrid actor started, initial mode = {} (CTC_POWERSAVE={})",
                    initial_mode, initial_powersave
                );
                background_tasks.push(join);
                Some(handle)
            }
            Err(e) => {
                error!(
                    "Failed to start SmartGrid actor: {e} — SmartGrid endpoints will return ServiceUnavailable"
                );
                None
            }
        }
    } else {
        debug!("GPIO control disabled - SmartGrid endpoints will return ServiceUnavailable");
        None
    };

    // Start price fetch background task if enabled
    if config.price.enabled {
        info!(
            "Price tracking enabled: zone={} (fetch daily ~{:02}:00 {})",
            config.price.zone, config.price.fetch_hour_local, config.tz
        );

        let price_state_clone = price_state.clone();
        let price_zone = config.price.zone.clone();
        let fetch_hour_local = config.price.fetch_hour_local;
        let cancel_for_price = cancel.clone();

        background_tasks.push(supervisor::spawn_with_shutdown(
            "price-fetch",
            cancel.clone(),
            async move {
                run_price_fetch_loop(
                    price_state_clone,
                    price_zone,
                    fetch_hour_local,
                    tz,
                    cancel_for_price,
                )
                .await;
            },
        ));
    } else {
        debug!("Price tracking disabled");
    }

    let app = Router::new()
        // .route("/ctc", get(ctx_handler))
        .merge(routes::temperatures::routes(
            store.clone(),
            tx.clone(),
            config.temperature_validation.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::ctc::routes(
            store.clone(),
            tx.clone(),
            config.modbus.request_timeout_secs,
            smartgrid_handle.clone(),
            price_state.clone(),
            config.smartgrid.clone(),
        ))
        .merge(routes::smartgrid::routes(
            smartgrid_handle,
            price_state.clone(),
            config.smartgrid.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::visibility::routes(
            tx.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::modbus::routes(
            tx.clone(),
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::alarms::routes(
            tx,
            config.modbus.request_timeout_secs,
        ))
        .merge(routes::grid::routes(grid_state, price_state))
        .merge(routes::heatpump_stats::routes(heatpump_stats.clone()))
        .merge(routes::series::routes(store.clone()))
        .merge(routes::activity::routes(store.clone()))
        .merge(routes::step_response::routes(store.clone()))
        .route(
            "/api/v1/version",
            get(|| async { concat!("{\"version\": \"", env!("CARGO_PKG_VERSION"), "\"}\n") }),
        )
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

    // Handle SIGTERM so `docker compose stop|down|restart` triggers the final flush.
    let store_for_shutdown = store.clone();
    let cancel_for_shutdown = cancel.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("Shutdown signal received — cancelling background tasks");
            cancel_for_shutdown.cancel();
            // 5 s is generous next to the longest operation (Modbus read ~1 s,
            // elpris HTTP fetch ~5 s with our timeout).
            let shutdown_wait = Duration::from_secs(5);
            for mut handle in background_tasks {
                // Dropping a JoinHandle detaches the task rather than cancelling it,
                // so keep an abort handle for tasks that overshoot the deadline.
                let abort = handle.abort_handle();
                match tokio::time::timeout(shutdown_wait, &mut handle).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => error!("Background task ended with error: {e}"),
                    Err(_) => {
                        warn!("Background task did not exit within {shutdown_wait:?} — aborting");
                        abort.abort();
                        let _ = handle.await;
                    }
                }
            }
            info!("Background tasks settled — flushing store");
            if let Err(e) = store_for_shutdown.flush() {
                error!("Final store flush failed: {e}");
            }
        })
        .await?;

    Ok(())
}

/// Wait for SIGINT (Ctrl-C) or SIGTERM (`docker compose stop|down|restart`).
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGTERM handler: {e}");
            return;
        }
    };
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to install SIGINT handler: {e}");
            return;
        }
    };
    tokio::select! {
        _ = sigterm.recv() => info!("SIGTERM received"),
        _ = sigint.recv()  => info!("SIGINT received"),
    }
}

/// elpris-fetch schedule constants.
///
/// elpris publishes once per day around 13:00 Swedish-local. We fetch at
/// `fetch_hour_local` (default 14:00, one-hour cushion), and on failure
/// retry every 15 min until both today + tomorrow populate or local
/// midnight, whichever comes first.
const PRICE_FETCH_RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Background task for fetching electricity spot prices from elprisetjustnu.se.
///
/// On startup we fetch immediately; on the daily schedule we sleep until the
/// next local `fetch_hour_local`. Failures retry every 15 min until both
/// today + tomorrow populate or local midnight.
async fn run_price_fetch_loop(
    price_state: PriceState,
    price_zone: String,
    fetch_hour_local: u32,
    tz: chrono_tz::Tz,
    cancel: tokio_util::sync::CancellationToken,
) {
    use crate::energy::elpris::ElprisClient;
    let elpris_client = ElprisClient::new(&price_zone, tz);

    info!("Price fetch loop started (daily ~{fetch_hour_local}:00 {tz})");

    // Initial fetch + retry-until-tomorrow-or-fetch-hour.
    if cancellable_sleep_until_fetch_done(
        &elpris_client,
        &price_state,
        fetch_hour_local,
        tz,
        &cancel,
    )
    .await
    {
        return;
    }

    loop {
        let now = SystemTime::now();
        let next = next_fire_at_local_hour_in(now, fetch_hour_local, tz);
        let wait = next.duration_since(now).unwrap_or(Duration::ZERO);
        info!("Next price fetch in {:?}", wait);
        if cancellable_sleep(&cancel, wait).await {
            info!("Price fetch loop: shutdown signal received");
            return;
        }

        if cancellable_sleep_until_fetch_done(
            &elpris_client,
            &price_state,
            fetch_hour_local,
            tz,
            &cancel,
        )
        .await
        {
            return;
        }
    }
}

/// Fetch today + tomorrow. Returns `true` if the cancellation token fired
/// before the call returned (caller should exit). Retries every 15 min on
/// failure until both vectors populate, or until local midnight.
async fn cancellable_sleep_until_fetch_done(
    elpris_client: &crate::energy::elpris::ElprisClient,
    price_state: &PriceState,
    fetch_hour_local: u32,
    tz: chrono_tz::Tz,
    cancel: &tokio_util::sync::CancellationToken,
) -> bool {
    loop {
        info!("Fetching electricity prices...");
        let (today_result, tomorrow_result) = tokio::join!(
            elpris_client.get_today_prices(),
            elpris_client.try_get_tomorrow_prices(),
        );
        let fresh_today = match today_result {
            Ok(prices) => {
                info!("Fetched {} spot prices for today", prices.len());
                elpris_to_points(&prices)
            }
            Err(e) => {
                warn!("Failed to fetch today's spot prices; keeping previously-cached values: {e}");
                Vec::new()
            }
        };
        let fresh_tomorrow = match tomorrow_result {
            Some(prices) => {
                info!("Fetched {} spot prices for tomorrow", prices.len());
                elpris_to_points(&prices)
            }
            None => Vec::new(),
        };

        // Empty-fresh means either a transient failure OR "tomorrow not yet
        // published". Either way, preserving the previously-cached vector is
        // safer than wiping it — a 5-min network blip should not blank the
        // SmartGrid scheduler's price view until the next 14:00 fetch.
        let today_fresh = !fresh_today.is_empty();
        let tomorrow_fresh = !fresh_tomorrow.is_empty();
        let both_populated = today_fresh && tomorrow_fresh;

        let today_points = if today_fresh {
            calculate_price_levels(fresh_today)
        } else {
            price_state.get_today()
        };
        let tomorrow_points = if tomorrow_fresh {
            calculate_price_levels(fresh_tomorrow)
        } else {
            price_state.get_tomorrow()
        };
        price_state.update_prices(today_points, tomorrow_points);
        info!("Price state updated");

        if both_populated {
            return false;
        }

        // Stop retrying past local midnight — at that point today rolls over
        // and the next scheduled fetch handles it.
        let now = SystemTime::now();
        let (_, _, _, hour) = crate::energy::tariff::system_time_to_local(now, tz);
        if hour < fetch_hour_local {
            // Pre-fetch-hour retry path: only one vector missing (typically
            // tomorrow not yet published). Wait for the scheduled fetch
            // rather than spamming.
            return false;
        }
        let next_midnight = next_local_midnight_in(now, tz);
        let until_midnight = next_midnight.duration_since(now).unwrap_or(Duration::ZERO);
        if until_midnight <= PRICE_FETCH_RETRY_INTERVAL {
            error!(
                "Price fetch still incomplete at end of day — leaving state with whatever populated"
            );
            return false;
        }

        warn!(
            "Price fetch incomplete; retrying in {:?}",
            PRICE_FETCH_RETRY_INTERVAL
        );
        if cancellable_sleep(cancel, PRICE_FETCH_RETRY_INTERVAL).await {
            info!("Price fetch loop: shutdown signal received during retry sleep");
            return true;
        }
    }
}

/// Compute the next `target_hour:00` in `tz` strictly after `now`.
fn next_fire_at_local_hour_in(now: SystemTime, target_hour: u32, tz: chrono_tz::Tz) -> SystemTime {
    use chrono::{DateTime, TimeZone, Utc};
    let utc_now: DateTime<Utc> = now.into();
    let local_now = utc_now.with_timezone(&tz);

    // Try today's target hour first; if it's already past, advance one local day.
    let mut candidate_date = local_now.date_naive();
    for _ in 0..2 {
        let naive = candidate_date
            .and_hms_opt(target_hour, 0, 0)
            .expect("0..=23 is a valid hour");
        if let Some(local) = tz.from_local_datetime(&naive).earliest()
            && local > local_now
        {
            return local.with_timezone(&Utc).into();
        }
        candidate_date += chrono::Duration::days(1);
    }
    // Fallback (e.g. spring-forward gap consumed both candidates): use the
    // pure "+ 1 local day" path. In practice fetch_hour_local is well outside
    // the 02:00-03:00 gap, so this is defensive only.
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(next_local_midnight_secs_in(now, tz) + u64::from(target_hour) * 3600)
}

/// `SystemTime` of the next local midnight in `tz` strictly after `now`.
fn next_local_midnight_in(now: SystemTime, tz: chrono_tz::Tz) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(next_local_midnight_secs_in(now, tz))
}

fn next_local_midnight_secs_in(now: SystemTime, tz: chrono_tz::Tz) -> u64 {
    use chrono::{DateTime, TimeZone, Utc};
    let utc_now: DateTime<Utc> = now.into();
    let local_now = utc_now.with_timezone(&tz);
    let next_midnight_naive = (local_now.date_naive() + chrono::Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .expect("00:00 is a valid time-of-day");
    let local = tz
        .from_local_datetime(&next_midnight_naive)
        .earliest()
        .expect("local midnight is unambiguous in practice");
    #[allow(clippy::cast_sign_loss)]
    {
        local.timestamp() as u64
    }
}

/// Sleep for `delay`, returning `true` if cancellation fires before the
/// timeout elapses.
async fn cancellable_sleep(cancel: &tokio_util::sync::CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(delay) => false,
    }
}

/// Convert elpris entries to `PricePoint`s (no level yet).
fn elpris_to_points(
    elpris: &[crate::energy::elpris::ElprisEntry],
) -> Vec<crate::energy::price::PricePoint> {
    use crate::energy::price::PricePoint;
    elpris
        .iter()
        .map(|e| {
            PricePoint::from_spot(
                e.time_start.clone(),
                e.time_end.clone(),
                e.sek_per_kwh,
                e.eur_per_kwh,
                e.exchange_rate,
            )
        })
        .collect()
}

/// Calculate price levels from spot-price percentiles.
///
/// Uses value-based bins (not position-based ranking) so that ties land in the
/// same bin. Cutoffs at p25, p40, p60, p75 split the five levels
/// (`VeryCheap`, `Cheap`, `Normal`, `Expensive`, `VeryExpensive`).
///
/// Comparisons are strict `<`. Ties at the top percentile all fall into
/// `VeryExpensive` (`v < p75` is false for all values equal to `p75`).
/// A single-element input also lands in `VeryExpensive` for the same reason.
/// When all values are equal the short-circuit returns `Normal`.
#[allow(clippy::cast_precision_loss)]
fn calculate_price_levels(
    mut prices: Vec<crate::energy::price::PricePoint>,
) -> Vec<crate::energy::price::PricePoint> {
    use crate::energy::price::PriceLevel;

    if prices.is_empty() {
        return prices;
    }

    // Compute percentile cutoff values once.
    let mut sorted_prices: Vec<f64> = prices.iter().map(|p| p.spot_sek).collect();
    sorted_prices.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Degenerate case: all prices equal (e.g. all zero on a calm weekend).
    // Treat them all as Normal — any other classification is meaningless.
    //
    // Tolerance is half an öre/kWh: spot prices are reported to 5-6 decimals
    // but the user-meaningful resolution is öre (0.01 SEK/kWh). A spread
    // below half an öre is rounding noise, not a real price gradient.
    const ORE_TOLERANCE_SEK_PER_KWH: f64 = 0.005;
    if (sorted_prices.last().copied().unwrap_or(0.0)
        - sorted_prices.first().copied().unwrap_or(0.0))
    .abs()
        < ORE_TOLERANCE_SEK_PER_KWH
    {
        for price in &mut prices {
            price.level = Some(PriceLevel::Normal);
        }
        return prices;
    }

    let p25 = percentile_value(&sorted_prices, 0.25);
    let p40 = percentile_value(&sorted_prices, 0.40);
    let p60 = percentile_value(&sorted_prices, 0.60);
    let p75 = percentile_value(&sorted_prices, 0.75);

    for price in &mut prices {
        let v = price.spot_sek;
        price.level = Some(if v < p25 {
            PriceLevel::VeryCheap
        } else if v < p40 {
            PriceLevel::Cheap
        } else if v < p60 {
            PriceLevel::Normal
        } else if v < p75 {
            PriceLevel::Expensive
        } else {
            PriceLevel::VeryExpensive
        });
    }

    prices
}

/// Return the value at the given percentile (0.0..=1.0) of a sorted slice.
/// Uses nearest-rank: index = floor(p * n), clamped to the slice.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn percentile_value(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::INFINITY;
    }
    let n = sorted.len();
    let idx = ((p * n as f64) as usize).min(n - 1);
    sorted[idx]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::energy::price::{PriceLevel, PricePoint};

    fn point(spot: f64) -> PricePoint {
        PricePoint::from_spot(String::new(), String::new(), spot, 0.0, 0.0)
    }

    #[test]
    fn calculate_price_levels_top_ties_become_very_expensive() {
        // n=4 with two ties at the max. Strict `<` semantics mean both 3.0
        // values land in VeryExpensive (because `3.0 < p75=3.0` is false).
        let prices = calculate_price_levels(vec![point(1.0), point(2.0), point(3.0), point(3.0)]);
        let levels: Vec<_> = prices.iter().map(|p| p.level).collect();
        assert_eq!(levels[0], Some(PriceLevel::VeryCheap));
        assert_eq!(levels[1], Some(PriceLevel::Normal));
        assert_eq!(levels[2], Some(PriceLevel::VeryExpensive));
        assert_eq!(levels[3], Some(PriceLevel::VeryExpensive));
    }

    #[test]
    fn calculate_price_levels_single_value_is_normal() {
        // n=1: min == max → all-equal short-circuit returns Normal.
        let prices = calculate_price_levels(vec![point(0.5)]);
        assert_eq!(prices[0].level, Some(PriceLevel::Normal));
    }

    #[test]
    fn calculate_price_levels_all_zero_is_normal() {
        let prices = calculate_price_levels(vec![point(0.0), point(0.0), point(0.0), point(0.0)]);
        for p in &prices {
            assert_eq!(p.level, Some(PriceLevel::Normal));
        }
    }

    #[test]
    fn calculate_price_levels_within_ore_tolerance_is_normal() {
        // Prices differing only at the 1e-9 SEK/kWh level are effectively
        // identical from a user / SmartGrid standpoint. Without a real
        // tolerance, a spread of 1e-10 SEK/kWh would skip the all-equal
        // short-circuit and fall into percentile binning, producing different
        // levels for what is genuinely the same price.
        let prices = calculate_price_levels(vec![
            point(0.500_000_000_1),
            point(0.500_000_000_2),
            point(0.500_000_000_3),
            point(0.500_000_000_4),
        ]);
        for p in &prices {
            assert_eq!(p.level, Some(PriceLevel::Normal));
        }
    }

    #[test]
    fn next_fire_at_14_skips_to_tomorrow_when_past_today() {
        use crate::energy::tariff::{local_midnight_utc_secs, system_time_to_local};
        use chrono_tz::Europe::Stockholm;
        // 2026-01-15 (Thursday). 15:00 Swedish-local CET = 14:00 UTC.
        let local_midnight = local_midnight_utc_secs(2026, 1, 15, Stockholm);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(local_midnight + 15 * 3600);
        let next = next_fire_at_local_hour_in(now, 14, Stockholm);
        let (year, month, day, hour) = system_time_to_local(next, Stockholm);
        // Past today's 14:00 → fire is tomorrow's 14:00 local.
        assert_eq!((year, month, day, hour), (2026, 1, 16, 14));
    }

    #[test]
    fn next_fire_at_14_uses_today_when_morning() {
        use crate::energy::tariff::{local_midnight_utc_secs, system_time_to_local};
        use chrono_tz::Europe::Stockholm;
        let local_midnight = local_midnight_utc_secs(2026, 1, 15, Stockholm);
        // 09:00 Swedish-local → still before 14:00, fire is today's 14:00.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(local_midnight + 9 * 3600);
        let next = next_fire_at_local_hour_in(now, 14, Stockholm);
        let (year, month, day, hour) = system_time_to_local(next, Stockholm);
        assert_eq!((year, month, day, hour), (2026, 1, 15, 14));
    }

    /// Configurable hour: a different target hour (e.g. 16) should fire at
    /// that hour, not at the previous default of 14.
    #[test]
    fn next_fire_at_custom_hour_respects_parameter() {
        use crate::energy::tariff::{local_midnight_utc_secs, system_time_to_local};
        use chrono_tz::Europe::Stockholm;
        let local_midnight = local_midnight_utc_secs(2026, 1, 15, Stockholm);
        // 09:00 Swedish-local — well before any sane fetch hour.
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(local_midnight + 9 * 3600);
        let next_16 = next_fire_at_local_hour_in(now, 16, Stockholm);
        let (y, m, d, h) = system_time_to_local(next_16, Stockholm);
        assert_eq!((y, m, d, h), (2026, 1, 15, 16));

        // And the boundary case: target hour = 0 (midnight) should fire at
        // tomorrow's midnight (since today's midnight is already past).
        let next_0 = next_fire_at_local_hour_in(now, 0, Stockholm);
        let (y0, m0, d0, h0) = system_time_to_local(next_0, Stockholm);
        assert_eq!((y0, m0, d0, h0), (2026, 1, 16, 0));
    }

    /// Fall-back day: 14:00 local is unambiguous (the second 02:00 is the
    /// only ambiguous hour) and lands at 13:00 UTC. The chrono-tz-backed
    /// helper resolves it correctly, so firing happens at local 14:00.
    #[test]
    fn next_fire_at_14_fall_back_day_fires_at_local_14() {
        use crate::energy::tariff::{local_midnight_utc_secs, system_time_to_local};
        use chrono_tz::Europe::Stockholm;
        let oct26 = local_midnight_utc_secs(2025, 10, 26, Stockholm);
        // 00:30 UTC == 02:30 CEST (pre fall-back).
        let pre = SystemTime::UNIX_EPOCH + Duration::from_secs(oct26 + 30 * 60);
        let next_pre = next_fire_at_local_hour_in(pre, 14, Stockholm);
        let (y, m, d, h) = system_time_to_local(next_pre, Stockholm);
        assert_eq!((y, m, d, h), (2025, 10, 26, 14));
    }

    #[test]
    fn next_fire_at_14_handles_spring_forward() {
        use crate::energy::tariff::{local_midnight_utc_secs, system_time_to_local};
        use chrono_tz::Europe::Stockholm;
        // 2026-03-28 (Saturday before spring forward) 09:00 Swedish-local CET.
        // The fire should land on 2026-03-28 14:00 local (still CET).
        let mar28 = local_midnight_utc_secs(2026, 3, 28, Stockholm);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(mar28 + 9 * 3600);
        let next = next_fire_at_local_hour_in(now, 14, Stockholm);
        let (year, month, day, hour) = system_time_to_local(next, Stockholm);
        assert_eq!((year, month, day, hour), (2026, 3, 28, 14));

        // 2026-03-29 (spring-forward Sunday) 15:00 Swedish-local CEST. Next
        // fire is 2026-03-30 14:00 local — CEST, which the DST-aware helper
        // must produce.
        let mar29 = local_midnight_utc_secs(2026, 3, 29, Stockholm);
        // CET midnight + 16h is 14:00 CEST (DST jumped at 02:00 local).
        // Use a SystemTime well after the jump.
        let now2 = SystemTime::UNIX_EPOCH + Duration::from_secs(mar29 + 16 * 3600);
        let next2 = next_fire_at_local_hour_in(now2, 14, Stockholm);
        let (y2, m2, d2, h2) = system_time_to_local(next2, Stockholm);
        assert_eq!((y2, m2, d2, h2), (2026, 3, 30, 14));
    }

    #[test]
    fn calculate_price_levels_negative_spot_populates() {
        // Nord Pool can publish negative spot prices. Levels must still bin
        // across the full negative-to-positive range.
        let prices = calculate_price_levels(vec![point(-0.5), point(-0.1), point(0.2), point(0.8)]);
        // All four levels must be assigned (no None).
        for p in &prices {
            assert!(p.level.is_some(), "negative spot left level=None");
        }
        // Smallest must be the cheapest bin, largest the most expensive.
        assert_eq!(prices[0].level, Some(PriceLevel::VeryCheap));
        assert_eq!(prices[3].level, Some(PriceLevel::VeryExpensive));
    }
}
