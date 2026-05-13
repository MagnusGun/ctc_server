//! Tibber WebSocket client for real-time live measurements
//!
//! Connects to Tibber's WebSocket API to receive real-time power consumption
//! data from Tibber Pulse devices.

use std::time::{Duration, SystemTime};

use chrono_tz::Tz;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

use super::grid::GridState;
use super::tariff::{TariffMode, get_tariff_at, system_time_to_local};

const TIBBER_WS_URL: &str = "wss://websocket-api.tibber.com/v1-beta/gql/subscriptions";
const TIBBER_API_URL: &str = "https://api.tibber.com/v1-beta/gql";
const USER_AGENT: &str = "CTC-Server/1.0";
/// Read timeout for WebSocket messages. Tibber sends keep-alive every ~30-60s,
/// so 120s allows 2-4 missed keep-alives before declaring connection dead.
const WS_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// GraphQL subscription message format
#[derive(Debug, Serialize)]
struct GraphQLSubscription {
    #[serde(rename = "type")]
    msg_type: String,
    id: String,
    payload: SubscriptionPayload,
}

#[derive(Debug, Serialize)]
struct SubscriptionPayload {
    query: String,
    variables: serde_json::Value,
}

/// WebSocket message from Tibber
#[derive(Debug, Deserialize)]
struct WSMessage {
    #[serde(rename = "type")]
    msg_type: String,
    payload: Option<serde_json::Value>,
}

/// Live measurement data wrapper
#[derive(Debug, Deserialize)]
struct LiveMeasurementData {
    data: Option<LiveMeasurementResponse>,
}

#[derive(Debug, Deserialize)]
struct LiveMeasurementResponse {
    #[serde(rename = "liveMeasurement")]
    live_measurement: Option<LiveMeasurement>,
}

#[derive(Debug, Deserialize)]
struct LiveMeasurement {
    timestamp: String,
    power: Option<f32>,
    #[serde(rename = "accumulatedConsumption")]
    accumulated_consumption: Option<f32>,
    #[serde(rename = "accumulatedConsumptionLastHour")]
    accumulated_consumption_last_hour: Option<f32>,
}

/// Historical consumption API response
#[derive(Deserialize)]
struct ConsumptionResponse {
    data: Option<ConsumptionData>,
}

#[derive(Deserialize)]
struct ConsumptionData {
    viewer: ConsumptionViewer,
}

#[derive(Deserialize)]
struct ConsumptionViewer {
    home: Option<ConsumptionHome>,
}

#[derive(Deserialize)]
struct ConsumptionHome {
    consumption: Option<Consumption>,
}

#[derive(Deserialize)]
struct Consumption {
    nodes: Vec<ConsumptionNode>,
}

#[derive(Deserialize)]
struct ConsumptionNode {
    from: String,
    consumption: Option<f32>,
}

/// Home ID API response
#[derive(Deserialize)]
struct HomeResponse {
    data: HomeData,
}

#[derive(Deserialize)]
struct HomeData {
    viewer: HomeViewer,
}

#[derive(Deserialize)]
struct HomeViewer {
    homes: Vec<Home>,
}

#[derive(Deserialize)]
struct Home {
    id: String,
}

/// Run the WebSocket connection loop with automatic reconnection
///
/// Syncs historical data ONCE at startup, then relies on real-time WebSocket
/// updates. The grid state's `maybe_update_peak()` method handles updating
/// daily peaks when the current hour exceeds stored values.
pub async fn run_websocket_loop(
    api_token: String,
    grid_state: GridState,
    tz: Tz,
    cancel: CancellationToken,
) {
    // Sync historical data ONCE at startup. Bail early on cancellation so the
    // (potentially slow) HTTP call doesn't hold up shutdown.
    info!("Tibber WS: Syncing historical consumption data (startup only)...");
    tokio::select! {
        biased;
        () = cancel.cancelled() => {
            info!("Tibber WS: shutdown signal received during startup sync");
            return;
        }
        result = sync_historical_data(&api_token, &grid_state, tz) => {
            if let Err(e) = result {
                error!("Tibber WS: Failed to sync historical data: {}", e);
            }
        }
    }

    // Exponential backoff for connection failures, capped at 5 minutes.
    // Resets to the base delay after any successful connect.
    let base_delay = Duration::from_secs(10);
    let max_delay = Duration::from_secs(300);
    let mut delay = base_delay;
    loop {
        if cancel.is_cancelled() {
            info!("Tibber WS: shutdown signal received");
            return;
        }
        info!("Tibber WS: Connecting to Tibber WebSocket...");

        let connect_result = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Tibber WS: shutdown signal received mid-connect");
                return;
            }
            r = connect_websocket(&api_token, &grid_state) => r,
        };

        match connect_result {
            Ok(()) => {
                warn!("Tibber WS: Connection closed, reconnecting in 10 seconds...");
                delay = base_delay;
                if cancellable_sleep(&cancel, delay).await {
                    return;
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                // 401 means the token is bad — exponential backoff won't fix
                // that. Stop the loop so the operator notices via /status.
                if msg.contains("401") || msg.to_lowercase().contains("unauthorized") {
                    error!("Tibber WS: 401 Unauthorized — aborting reconnect loop: {msg}");
                    return;
                }
                error!("Tibber WS: Connection error: {msg}");
                info!("Tibber WS: Reconnecting in {:?}", delay);
                if cancellable_sleep(&cancel, delay).await {
                    return;
                }
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}

/// Sleep for `delay`, or return early if cancellation fires first.
/// Returns `true` when cancelled, `false` on normal wake-up.
async fn cancellable_sleep(cancel: &CancellationToken, delay: Duration) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = time::sleep(delay) => false,
    }
}

/// Sync historical consumption data for the current month
async fn sync_historical_data(
    api_token: &str,
    grid_state: &GridState,
    tz: Tz,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use chrono::Datelike;
    let home_id = get_home_id(api_token).await?;

    // Fetch max allowed (744 hours = 31 days), will filter by month.
    // The filter compares the local (year, month) of each consumption node
    // against the current local month; route the current month through the
    // DST-aware helper so it rolls over at the correct local instant.
    let (current_year, current_month, _, _) = system_time_to_local(SystemTime::now(), tz);
    let hours_to_fetch = 744; // Max allowed by API

    info!(
        "Tibber WS: Fetching {} hours of historical data for home {} (filtering to {}-{:02})",
        hours_to_fetch, home_id, current_year, current_month
    );

    let query = format!(
        r#"{{
            viewer {{
                home(id: "{home_id}") {{
                    consumption(resolution: HOURLY, last: {hours_to_fetch}) {{
                        nodes {{
                            from
                            consumption
                        }}
                    }}
                }}
            }}
        }}"#
    );

    let client = crate::energy::http_client();
    let response = client
        .post(TIBBER_API_URL)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()).into());
    }

    let response: ConsumptionResponse = response.json().await?;
    let nodes = response
        .data
        .and_then(|d| d.viewer.home)
        .and_then(|h| h.consumption)
        .map(|c| c.nodes)
        .unwrap_or_default();

    // Log data range for debugging
    if let Some(first) = nodes.first() {
        let last_from = nodes.last().map_or(&first.from, |n| &n.from);
        trace!("Tibber WS: Data range: {} to {}", first.from, last_from);
    }

    let mut recorded = 0;
    let mut skipped_low_tariff = 0;
    let mut skipped_no_data = 0;
    let mut skipped_wrong_month = 0;

    for node in &nodes {
        let Some(kwh) = node.consumption else {
            skipped_no_data += 1;
            continue;
        };
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(node.from.trim()) else {
            continue;
        };
        if parsed.year() != current_year || parsed.month() != current_month {
            skipped_wrong_month += 1;
            continue;
        }
        let utc_secs = parsed.timestamp();
        if utc_secs < 0 {
            continue;
        }
        #[allow(clippy::cast_sign_loss)]
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(utc_secs as u64);
        if get_tariff_at(timestamp) == TariffMode::High {
            grid_state.record_hour(timestamp, f64::from(kwh));
            recorded += 1;
        } else {
            skipped_low_tariff += 1;
        }
    }

    info!(
        "Tibber WS: Historical sync complete - {} high-tariff recorded, {} low-tariff skipped, {} wrong month, {} no data",
        recorded, skipped_low_tariff, skipped_wrong_month, skipped_no_data
    );

    Ok(())
}

/// Get the home ID from Tibber API
async fn get_home_id(api_token: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = crate::energy::http_client();
    let query = r"{ viewer { homes { id } } }";

    let response = client
        .post(TIBBER_API_URL)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await?;

    let home_response: HomeResponse = response.json().await?;
    home_response
        .data
        .viewer
        .homes
        .first()
        .map(|h| h.id.clone())
        .ok_or_else(|| "No homes found in Tibber account".into())
}

/// Connect to Tibber WebSocket and process messages
#[allow(clippy::too_many_lines)]
async fn connect_websocket(
    api_token: &str,
    grid_state: &GridState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home_id = get_home_id(api_token).await?;
    info!("Tibber WS: Using home ID: {}", home_id);

    // Build WebSocket request with required headers
    let mut request = TIBBER_WS_URL.into_client_request()?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        "graphql-transport-ws".parse().unwrap(),
    );
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {api_token}").parse().unwrap(),
    );
    request
        .headers_mut()
        .insert("User-Agent", USER_AGENT.parse().unwrap());

    let (ws_stream, _) = connect_async(request).await?;
    info!("Tibber WS: Connected");

    let (mut write, mut read) = ws_stream.split();

    // Send connection_init
    let init_msg = serde_json::json!({
        "type": "connection_init",
        "payload": {}
    });
    write
        .send(Message::Text(init_msg.to_string().into()))
        .await?;

    // Wait for connection_ack
    let mut connection_acked = false;
    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                let ws_msg: WSMessage = serde_json::from_str(&text)?;
                trace!("Tibber WS: Received message type: {}", ws_msg.msg_type);

                if ws_msg.msg_type == "connection_ack" {
                    info!("Tibber WS: Connection acknowledged");
                    connection_acked = true;
                    break;
                } else if ws_msg.msg_type == "ka" {
                    trace!("Tibber WS: Keep-alive");
                } else if ws_msg.msg_type == "connection_error" {
                    return Err(format!("Connection error: {:?}", ws_msg.payload).into());
                }
            }
            Ok(Message::Close(reason)) => {
                return Err(format!("WebSocket closed before connection_ack: {reason:?}").into());
            }
            Err(e) => {
                return Err(format!("WebSocket error: {e}").into());
            }
            _ => {}
        }
    }

    if !connection_acked {
        return Err("Stream ended before connection_ack".into());
    }

    // Subscribe to live measurements
    let subscription = GraphQLSubscription {
        msg_type: "subscribe".to_string(),
        id: "1".to_string(),
        payload: SubscriptionPayload {
            query: format!(
                r#"
                subscription {{
                    liveMeasurement(homeId: "{home_id}") {{
                        timestamp
                        power
                        accumulatedConsumption
                        accumulatedConsumptionLastHour
                    }}
                }}
                "#
            ),
            variables: serde_json::json!({}),
        },
    };

    let sub_msg = serde_json::to_string(&subscription)?;
    write.send(Message::Text(sub_msg.into())).await?;
    info!("Tibber WS: Subscribed to live measurements");

    // Per-connection rotation state. Both fields stay None until the first
    // measurement lands; that way the first message after a reconnect both
    // initialises the trackers AND records its quarter/hour normally instead
    // of silently skipping (audit #9).
    let mut ws_state = WsRotationState::default();

    // Process incoming messages with read timeout to detect zombie connections.
    loop {
        match time::timeout(WS_READ_TIMEOUT, read.next()).await {
            Ok(Some(Ok(msg))) => match msg {
                Message::Text(text) => {
                    if let Err(e) = process_message(&text, grid_state, &mut ws_state) {
                        error!("Tibber WS: Failed to process message: {}", e);
                    }
                }
                Message::Close(_) => {
                    info!("Tibber WS: Closed by server");
                    break;
                }
                Message::Ping(data) => {
                    let _ = write.send(Message::Pong(data)).await;
                }
                _ => {}
            },
            Ok(Some(Err(e))) => {
                error!("Tibber WS: Error: {}", e);
                break;
            }
            Ok(None) => {
                info!("Tibber WS: Stream ended");
                break;
            }
            Err(_) => {
                warn!("Tibber WS: No message in {WS_READ_TIMEOUT:?}, reconnecting");
                break;
            }
        }
    }

    Ok(())
}

/// State tracked between successive WS messages so we can detect quarter +
/// hour rollovers and the midnight reset of `accumulated_consumption`.
#[derive(Default)]
struct WsRotationState {
    /// Most recent hour boundary (Unix-secs floor to 3600) seen on the
    /// stream. Used by the hourly peak path.
    last_recorded_hour: Option<u64>,
    /// Most recent quarter boundary (Unix-secs floor to 900) seen on the
    /// stream. Used by the 15-min consumption path.
    quarter_start_secs: Option<u64>,
    /// `accumulatedConsumption` value at the start of `quarter_start_secs`.
    /// Subtracting from the current accumulated gives this quarter's kWh.
    quarter_start_accumulated: Option<f64>,
}

/// Process a WebSocket message
fn process_message(
    text: &str,
    grid_state: &GridState,
    state: &mut WsRotationState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_msg: WSMessage = serde_json::from_str(text)?;

    if ws_msg.msg_type == "next" {
        if let Some(payload) = ws_msg.payload {
            let measurement_data: LiveMeasurementData = serde_json::from_value(payload)?;

            if let Some(data) = measurement_data.data
                && let Some(measurement) = data.live_measurement
            {
                let power = measurement.power.unwrap_or(0.0);
                let accumulated = measurement.accumulated_consumption.unwrap_or(0.0);
                let last_hour = measurement.accumulated_consumption_last_hour.unwrap_or(0.0);

                trace!(
                    "Tibber WS: power={:.0}W, accumulated={:.2}kWh, last_hour={:.2}kWh",
                    power, accumulated, last_hour
                );

                let Ok(timestamp) = parse_iso8601(&measurement.timestamp) else {
                    return Ok(());
                };
                let now_secs = timestamp
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let current_hour = (now_secs / 3600) * 3600;
                let current_quarter = (now_secs / 900) * 900;

                // ---- Hourly path (feeds effektabonnemang peak tracking) ----
                let prev_kwh = grid_state.get_current_hour_kwh();
                grid_state.update_current_hour(f64::from(last_hour));

                if let Some(prev_hour) = state.last_recorded_hour
                    && current_hour > prev_hour
                {
                    let prev_hour_time = SystemTime::UNIX_EPOCH + Duration::from_secs(prev_hour);
                    if let Some(prev_kwh) = prev_kwh {
                        grid_state.record_hour(prev_hour_time, prev_kwh);
                    }
                }
                state.last_recorded_hour = Some(current_hour);

                // ---- 15-minute path (current_quarter_kwh + consumption_15min) ----
                // Tibber's accumulated counter is reported to ~3 decimals
                // (mWh). A backward stutter inside that resolution is sensor
                // noise, not a real reset, so the "same quarter, no reset"
                // and "real midnight reset" guards both tolerate up to
                // RESET_JITTER_KWH backward before re-anchoring.
                const RESET_JITTER_KWH: f64 = 0.01;
                let accumulated = f64::from(accumulated);
                match (state.quarter_start_secs, state.quarter_start_accumulated) {
                    (Some(prev_quarter), Some(prev_snapshot))
                        if prev_quarter == current_quarter
                            && accumulated + RESET_JITTER_KWH >= prev_snapshot =>
                    {
                        // Same quarter, no midnight reset. Forward the live
                        // delta so the dashboard sees a smooth in-quarter
                        // accumulation. Clamp the delta at 0 in case the
                        // backward stutter is positive but tiny.
                        let delta = (accumulated - prev_snapshot).max(0.0);
                        grid_state.update_current_quarter(delta);
                    }
                    (Some(prev_quarter), Some(prev_snapshot)) => {
                        // Either the quarter rolled over OR midnight reset
                        // the accumulator. Close the prior quarter with the
                        // best estimate of its delta, then re-anchor.
                        let prior_delta = if accumulated + RESET_JITTER_KWH >= prev_snapshot {
                            (accumulated - prev_snapshot).max(0.0)
                        } else {
                            // Midnight (or counter wobble beyond jitter) —
                            // the prior quarter's tail is whatever we last
                            // forwarded.
                            grid_state.get_current_quarter_kwh()
                        };
                        if prior_delta > 0.0 {
                            grid_state.record_quarter(prev_quarter, prior_delta);
                        }
                        // Reset for the new quarter. Forward 0 so the
                        // current-quarter readout starts fresh.
                        state.quarter_start_secs = Some(current_quarter);
                        state.quarter_start_accumulated = Some(accumulated);
                        grid_state.update_current_quarter(0.0);
                    }
                    _ => {
                        // First measurement after (re)connect. Initialise
                        // the snapshot at the current quarter boundary so
                        // the next message produces a sensible delta. Don't
                        // emit a partial value for this first sample.
                        state.quarter_start_secs = Some(current_quarter);
                        state.quarter_start_accumulated = Some(accumulated);
                        grid_state.update_current_quarter(0.0);
                    }
                }
            }
        }
    } else if ws_msg.msg_type == "ka" {
        // Keep-alive, ignore
    }

    Ok(())
}

/// Parse ISO 8601 / RFC 3339 timestamp to `SystemTime`.
///
/// Delegates to `chrono::DateTime::parse_from_rfc3339` to avoid the
/// fractional-seconds / offset-spelling pitfalls of a hand-rolled parser.
pub(crate) fn parse_iso8601(
    s: &str,
) -> Result<SystemTime, Box<dyn std::error::Error + Send + Sync>> {
    let s = s.trim();
    // RFC 3339 requires a timezone designator; if missing, treat as UTC
    // to preserve the historical behaviour of this helper.
    let dt = match chrono::DateTime::parse_from_rfc3339(s) {
        Ok(dt) => dt,
        Err(_) => chrono::DateTime::parse_from_rfc3339(&format!("{s}Z"))?,
    };
    let utc_secs = dt.timestamp();
    if utc_secs < 0 {
        return Err("Timestamp before Unix epoch".into());
    }
    #[allow(clippy::cast_sign_loss)]
    Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(utc_secs as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract hour from `SystemTime` (in UTC)
    fn get_utc_hour(time: SystemTime) -> u32 {
        let secs = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        ((secs % 86400) / 3600) as u32
    }

    #[test]
    fn test_parse_iso8601_utc() {
        // "14:30:00Z" is already UTC, so result should be 14:30 UTC
        let result = parse_iso8601("2025-01-15T14:30:00Z").unwrap();
        assert_eq!(get_utc_hour(result), 14);
    }

    #[test]
    fn test_parse_iso8601_positive_offset() {
        // "14:30:00+01:00" means 14:30 local time at UTC+1
        // Converting to UTC: 14:30 - 1:00 = 13:30 UTC
        let result = parse_iso8601("2025-01-15T14:30:00.000+01:00").unwrap();
        assert_eq!(get_utc_hour(result), 13);
    }

    #[test]
    fn test_parse_iso8601_swedish_winter_time() {
        // Swedish winter time is UTC+1
        // "09:00:00+01:00" = 08:00 UTC
        let result = parse_iso8601("2026-01-09T09:00:00.000+01:00").unwrap();
        assert_eq!(get_utc_hour(result), 8);
    }

    #[test]
    fn test_parse_iso8601_swedish_summer_time() {
        // Swedish summer time is UTC+2
        // "09:00:00+02:00" = 07:00 UTC
        let result = parse_iso8601("2026-07-09T09:00:00.000+02:00").unwrap();
        assert_eq!(get_utc_hour(result), 7);
    }

    #[test]
    fn test_parse_iso8601_negative_offset() {
        // US Eastern Standard Time is UTC-5
        // "14:30:00-05:00" = 14:30 + 5:00 = 19:30 UTC
        let result = parse_iso8601("2025-01-15T14:30:00.000-05:00").unwrap();
        assert_eq!(get_utc_hour(result), 19);
    }

    #[test]
    fn test_parse_iso8601_no_timezone() {
        // No timezone assumes UTC
        let result = parse_iso8601("2025-01-15T14:30:00").unwrap();
        assert_eq!(get_utc_hour(result), 14);
    }

    #[test]
    fn test_parse_iso8601_half_hour_offset() {
        // India Standard Time is UTC+5:30
        // "14:30:00+05:30" = 14:30 - 5:30 = 09:00 UTC
        let result = parse_iso8601("2025-01-15T14:30:00+05:30").unwrap();
        assert_eq!(get_utc_hour(result), 9);
    }

    /// Build a `WSMessage` JSON payload mirroring what Tibber sends so we
    /// can exercise `process_message` directly.
    fn ws_payload(timestamp: &str, accumulated: f32, last_hour: f32) -> String {
        serde_json::json!({
            "type": "next",
            "payload": {
                "data": {
                    "liveMeasurement": {
                        "timestamp": timestamp,
                        "power": 0.0,
                        "accumulatedConsumption": accumulated,
                        "accumulatedConsumptionLastHour": last_hour,
                    }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn process_message_quarter_rollover_records_prior_quarter() {
        let grid = GridState::new();
        let mut state = WsRotationState::default();

        // First quarter (10:00–10:15 UTC). First message initialises; second
        // forwards the in-quarter delta.
        process_message(
            &ws_payload("2026-05-10T10:00:00Z", 0.0, 0.0),
            &grid,
            &mut state,
        )
        .unwrap();
        process_message(
            &ws_payload("2026-05-10T10:05:00Z", 0.3, 0.0),
            &grid,
            &mut state,
        )
        .unwrap();
        // Cross into the next quarter (10:15–10:30). The boundary message
        // carries accumulated=0.5 — exactly the prior-quarter close value
        // — so the closed quarter records 0.5 kWh of consumption.
        process_message(
            &ws_payload("2026-05-10T10:15:00Z", 0.5, 0.0),
            &grid,
            &mut state,
        )
        .unwrap();

        let consumption = grid.get_consumption_15min();
        assert_eq!(consumption.len(), 1, "prior quarter should be recorded");
        let entry = &consumption[0];
        assert!(
            (entry.kwh - 0.5).abs() < 1e-6,
            "prior quarter kwh: expected 0.5, got {}",
            entry.kwh
        );

        // The current-quarter readout resets after rollover so the dashboard
        // doesn't show the closed quarter's total on the new quarter.
        let now = grid.get_current_quarter_kwh();
        assert!(
            now.abs() < 1e-6,
            "current-quarter reading should reset to 0 after rollover; got {now}"
        );
    }

    #[test]
    fn process_message_first_message_after_reconnect_does_not_skip_quarter() {
        // Audit #9 (reframed): the very first measurement after a WS
        // reconnect used to silently skip the rollover branch because
        // last_recorded_hour was None. Now both rotation paths initialise
        // on the first message instead of skipping it.
        let grid = GridState::new();
        let mut state = WsRotationState::default();

        // First message of a new connection mid-quarter. State must
        // initialise without aborting and without polluting
        // consumption_15min with a spurious prior-quarter entry.
        process_message(
            &ws_payload("2026-05-10T10:07:00Z", 0.42, 0.0),
            &grid,
            &mut state,
        )
        .unwrap();
        assert_eq!(
            state.last_recorded_hour,
            Some(10 * 3600 + ymd_unix(2026, 5, 10))
        );
        assert!(
            state.quarter_start_secs.is_some(),
            "quarter snapshot must initialise on first message"
        );
        assert!(
            grid.get_consumption_15min().is_empty(),
            "first message must not retroactively close any quarter"
        );

        // The next quarter boundary closes the in-progress quarter properly
        // — confirming the first message wasn't silently skipped.
        process_message(
            &ws_payload("2026-05-10T10:15:00Z", 0.6, 0.0),
            &grid,
            &mut state,
        )
        .unwrap();
        assert_eq!(grid.get_consumption_15min().len(), 1);
    }

    #[allow(clippy::cast_sign_loss)]
    fn ymd_unix(year: i32, month: u32, day: u32) -> u64 {
        // Small helper for tests only — converts a UTC date to its midnight
        // Unix seconds without touching the production code path. Test inputs
        // are all post-1970 so the i64→u64 cast is safe.
        use chrono::TimeZone;
        chrono::Utc
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp() as u64
    }
}
