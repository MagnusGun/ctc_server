//! Tibber WebSocket client for real-time live measurements and price data
//!
//! Connects to Tibber's WebSocket API to receive real-time power consumption
//! data from Tibber Pulse devices. Also provides REST API access for price
//! information with `QUARTER_HOURLY` (15-minute) resolution.

use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::time;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, trace, warn};

use super::grid::GridState;
use super::tariff::{TariffMode, get_tariff_at};

const TIBBER_WS_URL: &str = "wss://websocket-api.tibber.com/v1-beta/gql/subscriptions";
const TIBBER_API_URL: &str = "https://api.tibber.com/v1-beta/gql";
const USER_AGENT: &str = "CTC-Server/1.0";

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

// ============================================================================
// Price API types
// ============================================================================

/// Price API response wrapper
#[derive(Debug, Deserialize)]
struct PriceResponse {
    data: Option<PriceData>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct PriceData {
    viewer: PriceViewer,
}

#[derive(Debug, Deserialize)]
struct PriceViewer {
    homes: Vec<PriceHome>,
}

#[derive(Debug, Deserialize)]
struct PriceHome {
    #[serde(rename = "currentSubscription")]
    current_subscription: Option<Subscription>,
}

#[derive(Debug, Deserialize)]
struct Subscription {
    #[serde(rename = "priceInfo")]
    price_info: Option<PriceInfo>,
}

#[derive(Debug, Deserialize)]
struct PriceInfo {
    current: Option<TibberPrice>,
    today: Vec<TibberPrice>,
    tomorrow: Vec<TibberPrice>,
}

/// Single price point from Tibber API
#[derive(Debug, Clone, Deserialize)]
pub struct TibberPrice {
    /// Total price in SEK/kWh (what you pay)
    pub total: f64,
    /// Energy component in SEK/kWh
    pub energy: f64,
    /// Tax component in SEK/kWh
    pub tax: f64,
    /// Start time (ISO 8601)
    #[serde(rename = "startsAt")]
    pub starts_at: String,
    /// Price level classification
    pub level: Option<String>,
}

/// Fetched price data from Tibber
#[derive(Debug, Clone)]
pub struct TibberPriceData {
    /// Current price
    pub current: Option<TibberPrice>,
    /// Today's prices (up to 96 for 15-min resolution)
    pub today: Vec<TibberPrice>,
    /// Tomorrow's prices (empty if not available yet)
    pub tomorrow: Vec<TibberPrice>,
}

/// Fetch electricity prices from Tibber API
///
/// Uses `QUARTER_HOURLY` resolution for 15-minute price intervals.
/// Returns None if the API call fails or no data is available.
pub async fn fetch_prices(api_token: &str) -> Option<TibberPriceData> {
    let query = r"
        query GetPrices {
            viewer {
                homes {
                    currentSubscription {
                        priceInfo(resolution: QUARTER_HOURLY) {
                            current {
                                total
                                energy
                                tax
                                startsAt
                                level
                            }
                            today {
                                total
                                energy
                                tax
                                startsAt
                                level
                            }
                            tomorrow {
                                total
                                energy
                                tax
                                startsAt
                                level
                            }
                        }
                    }
                }
            }
        }
    ";

    let client = reqwest::Client::new();

    let response = match client
        .post(TIBBER_API_URL)
        .header("Authorization", format!("Bearer {api_token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            error!("Tibber price fetch failed: {}", e);
            return None;
        }
    };

    if !response.status().is_success() {
        error!("Tibber price API returned HTTP {}", response.status());
        return None;
    }

    let price_response: PriceResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to parse Tibber price response: {}", e);
            return None;
        }
    };

    if let Some(errors) = price_response.errors {
        for err in &errors {
            error!("Tibber GraphQL error: {}", err.message);
        }
        return None;
    }

    let data = price_response.data?;
    let home = data.viewer.homes.first()?;
    let subscription = home.current_subscription.as_ref()?;
    let price_info = subscription.price_info.as_ref()?;

    info!(
        "Tibber prices fetched: {} today, {} tomorrow",
        price_info.today.len(),
        price_info.tomorrow.len()
    );

    Some(TibberPriceData {
        current: price_info.current.clone(),
        today: price_info.today.clone(),
        tomorrow: price_info.tomorrow.clone(),
    })
}

/// Run the WebSocket connection loop with automatic reconnection
///
/// Syncs historical data ONCE at startup, then relies on real-time WebSocket
/// updates. The grid state's `maybe_update_peak()` method handles updating
/// daily peaks when the current hour exceeds stored values.
pub async fn run_websocket_loop(api_token: String, grid_state: GridState) {
    // Sync historical data ONCE at startup
    info!("Tibber WS: Syncing historical consumption data (startup only)...");
    if let Err(e) = sync_historical_data(&api_token, &grid_state).await {
        error!("Tibber WS: Failed to sync historical data: {}", e);
    }

    // Start WebSocket listener (no periodic sync - real-time updates handle peaks)
    loop {
        info!("Tibber WS: Connecting to Tibber WebSocket...");

        match connect_websocket(&api_token, &grid_state).await {
            Ok(()) => {
                warn!("Tibber WS: Connection closed, reconnecting in 10 seconds...");
                time::sleep(Duration::from_secs(10)).await;
            }
            Err(e) => {
                error!("Tibber WS: Connection error: {}", e);
                info!("Tibber WS: Reconnecting in 30 seconds...");
                time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

/// Sync historical consumption data for the current month
async fn sync_historical_data(
    api_token: &str,
    grid_state: &GridState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let home_id = get_home_id(api_token).await?;

    // Fetch max allowed (744 hours = 31 days), will filter by month
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH)?;
    let days = duration.as_secs() / 86400;
    let (current_year, current_month, _) = days_to_ymd(days);
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

    let client = reqwest::Client::new();
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
        if let Some(kwh) = node.consumption {
            // Filter to current month using local time from timestamp
            if let Some((year, month)) = extract_year_month(&node.from)
                && (year != current_year || month != current_month)
            {
                skipped_wrong_month += 1;
                continue;
            }

            // Parse ISO 8601 timestamp for tariff check
            if let Ok(timestamp) = parse_iso8601(&node.from) {
                // Check if this was a high-tariff hour
                if get_tariff_at(timestamp) == TariffMode::High {
                    grid_state.record_hour(timestamp, f64::from(kwh));
                    recorded += 1;
                } else {
                    skipped_low_tariff += 1;
                }
            }
        } else {
            skipped_no_data += 1;
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
    let client = reqwest::Client::new();
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
    write.send(Message::Text(init_msg.to_string().into())).await?;

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

    // Track the last hour we recorded
    let mut last_recorded_hour: Option<u64> = None;

    // Process incoming messages
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Err(e) = process_message(&text, grid_state, &mut last_recorded_hour) {
                    error!("Tibber WS: Failed to process message: {}", e);
                }
            }
            Ok(Message::Close(_)) => {
                info!("Tibber WS: Closed by server");
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Err(e) => {
                error!("Tibber WS: Error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

/// Process a WebSocket message
fn process_message(
    text: &str,
    grid_state: &GridState,
    last_recorded_hour: &mut Option<u64>,
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

                // Update current hour consumption (using last completed hour)
                grid_state.update_current_hour(f64::from(last_hour));

                // Check if we need to record the previous hour
                if let Ok(timestamp) = parse_iso8601(&measurement.timestamp) {
                    let now_secs = timestamp
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let current_hour = (now_secs / 3600) * 3600;

                    if let Some(last_hour) = *last_recorded_hour
                        && current_hour > last_hour
                    {
                        // Hour changed - record the previous hour
                        let prev_hour_time =
                            SystemTime::UNIX_EPOCH + Duration::from_secs(last_hour);
                        // Get the accumulated consumption at hour end
                        let prev_kwh = grid_state.get_current_hour_kwh();
                        if prev_kwh > 0.0 {
                            grid_state.record_hour(prev_hour_time, prev_kwh);
                        }
                    }
                    *last_recorded_hour = Some(current_hour);
                }
            }
        }
    } else if ws_msg.msg_type == "ka" {
        // Keep-alive, ignore
    }

    Ok(())
}

/// Extract (year, month) from ISO 8601 string like "2026-01-15T09:00:00+01:00"
/// Uses the local time from the string directly (handles DST correctly)
fn extract_year_month(iso: &str) -> Option<(i32, u32)> {
    // Format: "YYYY-MM-..."
    if iso.len() >= 7 {
        let year: i32 = iso.get(0..4)?.parse().ok()?;
        let month: u32 = iso.get(5..7)?.parse().ok()?;
        Some((year, month))
    } else {
        None
    }
}

/// Parse ISO 8601 timestamp to `SystemTime`
fn parse_iso8601(s: &str) -> Result<SystemTime, Box<dyn std::error::Error + Send + Sync>> {
    // Parse format: "2025-01-15T14:30:00.000+01:00" or "2025-01-15T13:30:00Z"
    // Simple parser for common formats

    // Remove fractional seconds and timezone for parsing
    let s = s.trim();

    // Try to find the date and time parts
    let date_time = if let Some(idx) = s.find('T') {
        let date_part = &s[..idx];
        let time_part = if let Some(plus_idx) = s[idx..].find('+') {
            &s[idx + 1..idx + plus_idx]
        } else if let Some(z_idx) = s[idx..].find('Z') {
            &s[idx + 1..idx + z_idx]
        } else if let Some(minus_idx) = s[idx + 1..].find('-') {
            &s[idx + 1..idx + 1 + minus_idx]
        } else {
            &s[idx + 1..]
        };

        // Remove fractional seconds
        let time_part = if let Some(dot_idx) = time_part.find('.') {
            &time_part[..dot_idx]
        } else {
            time_part
        };

        (date_part, time_part)
    } else {
        return Err("Invalid ISO 8601 format".into());
    };

    let parts: Vec<&str> = date_time.0.split('-').collect();
    if parts.len() != 3 {
        return Err("Invalid date format".into());
    }
    let year: i32 = parts[0].parse()?;
    let month: u32 = parts[1].parse()?;
    let day: u32 = parts[2].parse()?;

    let time_parts: Vec<&str> = date_time.1.split(':').collect();
    if time_parts.len() < 2 {
        return Err("Invalid time format".into());
    }
    let hour: u32 = time_parts[0].parse()?;
    let minute: u32 = time_parts[1].parse()?;
    let second: u32 = if time_parts.len() > 2 {
        time_parts[2].parse().unwrap_or(0)
    } else {
        0
    };

    // Convert to Unix timestamp (assuming UTC for simplicity)
    let days = ymd_to_days(year, month, day);

    #[allow(clippy::cast_sign_loss)]
    let secs =
        (days * 86400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)) as u64;

    Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

/// Convert (year, month, day) to days since Unix epoch
#[allow(clippy::similar_names)]
fn ymd_to_days(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let m = i64::from(if month <= 2 { month + 12 } else { month });

    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m - 3) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_468
}

/// Convert days since Unix epoch to (year, month, day)
#[allow(clippy::similar_names)]
fn days_to_ymd(days: u64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let year = if m <= 2 { y + 1 } else { y } as i32;

    #[allow(clippy::cast_possible_truncation)]
    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_iso8601_utc() {
        let result = parse_iso8601("2025-01-15T14:30:00Z");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_iso8601_with_offset() {
        let result = parse_iso8601("2025-01-15T14:30:00.000+01:00");
        assert!(result.is_ok());
    }

    #[test]
    fn test_days_to_ymd() {
        // 2025-01-15 is 20103 days after 1970-01-01
        let (y, m, d) = days_to_ymd(20103);
        assert_eq!(y, 2025);
        assert_eq!(m, 1);
        assert_eq!(d, 15);
    }

    #[test]
    fn test_ymd_to_days() {
        let days = ymd_to_days(2025, 1, 15);
        assert_eq!(days, 20103);
    }
}
