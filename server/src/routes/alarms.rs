//! Alarm and info message API endpoints
//!
//! These endpoints provide access to active alarms and info messages from the CTC system.
//! The system uses a two-tier approach:
//! - Quick status check (for polling): reads just the count register
//! - Full details: reads bitmasks and text buffers for active alarms/info

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use axum::{Router, extract::State, routing::get};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use tracing::{debug, error, warn};

use crate::error::ApiError;
use crate::messages::{
    ALARM_DESCRIPTIONS, ALARM_DESCRIPTIONS_SV, ALARM_TEXT_CACHE, ALARM_TRANSLATIONS, AlarmMessage,
    CachedAlarmText, cleanup_inactive_codes, decode_text_buffer, parse_alarm_text,
    record_first_seen, scan_bitmask,
};
use crate::modbus::bms_parameters::CTC_ALARM_INFO_COUNT;
use crate::modbus::{ModbusResponse, ModbusSender, ParameterOperation};

/// Text buffer transfer register (write alarm/info reference here)
const TEXT_BUFFER_TRANSFER_REG: u16 = 65100;
/// Text buffer start register (read 25 registers for 50 characters)
const TEXT_BUFFER_START_REG: u16 = 65101;
/// Number of text buffer registers (25 registers = 50 characters max)
const TEXT_BUFFER_COUNT: u16 = 25;

/// Info bitmask: 10 registers starting at 65060 (160 infos max)
const INFO_BITMASK_START: u16 = 65060;
const INFO_BITMASK_COUNT: u16 = 10;

/// Alarm bitmask: 50 registers starting at 65010 (800 alarms max)
const ALARM_BITMASK_START: u16 = 65010;
const ALARM_BITMASK_COUNT: u16 = 50;

/// State for alarm routes
#[derive(Clone)]
pub struct AlarmState {
    sender: ModbusSender,
    request_timeout_secs: u64,
}

/// Response for alarm status endpoint
#[derive(Serialize)]
struct AlarmStatusResponse {
    alarm_count: u8,
    info_count: u8,
    has_alarms: bool,
    has_infos: bool,
}

/// Full alarm response with all active alarms and infos
#[derive(Serialize)]
struct AlarmResponse {
    alarm_count: u8,
    info_count: u8,
    alarms: Vec<AlarmMessage>,
    infos: Vec<AlarmMessage>,
}

pub fn routes(sender: ModbusSender, request_timeout_secs: u64) -> Router {
    let state = AlarmState {
        sender,
        request_timeout_secs,
    };

    Router::new()
        .route("/api/v1/alarms/status", get(get_alarm_status))
        .route("/api/v1/alarms", get(get_alarms))
        .with_state(state)
}

/// Quick status check for polling
/// GET /api/v1/alarms/status
///
/// Returns just the alarm and info counts. Fast operation suitable for polling.
///
/// Response format:
/// ```json
/// {
///   "alarm_count": 1,
///   "info_count": 2,
///   "has_alarms": true,
///   "has_infos": true
/// }
/// ```
async fn get_alarm_status(State(state): State<AlarmState>) -> Result<String, ApiError> {
    debug!("get_alarm_status: START");

    // Read alarm/info count register (65001)
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((ParameterOperation::Read(CTC_ALARM_INFO_COUNT), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
            // Lower byte = alarm count, upper byte = info count
            #[allow(clippy::cast_possible_truncation)]
            #[allow(clippy::cast_sign_loss)]
            let raw_value = value as u16;
            let alarm_count = (raw_value & 0xFF) as u8;
            let info_count = ((raw_value >> 8) & 0xFF) as u8;

            debug!(
                "get_alarm_status: SUCCESS - alarms={}, infos={}",
                alarm_count, info_count
            );

            let response = AlarmStatusResponse {
                alarm_count,
                info_count,
                has_alarms: alarm_count > 0,
                has_infos: info_count > 0,
            };

            serde_json::to_string(&response)
                .map(|s| s + "\n")
                .map_err(|_| ApiError::InternalError)
        }
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("get_alarm_status: Unexpected RawRegisters response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!("get_alarm_status: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("get_alarm_status: Failed to receive response - {}", e);
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "get_alarm_status: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Get all active alarms and info messages with their text
/// GET /api/v1/alarms
///
/// Returns full details for all active alarms and info messages.
/// This is a slower operation that reads text buffers for each active alarm/info.
///
/// Response format:
/// ```json
/// {
///   "alarm_count": 1,
///   "info_count": 2,
///   "alarms": [
///     {
///       "reference": 40,
///       "code": "E040",
///       "message": "Low brine flow",
///       "message_en": "Low brine flow"
///     }
///   ],
///   "infos": [
///     {
///       "reference": 19,
///       "code": "I019",
///       "message": "Smart: Low price",
///       "message_en": "Smart: Low price"
///     }
///   ]
/// }
/// ```
#[allow(clippy::too_many_lines)]
async fn get_alarms(State(state): State<AlarmState>) -> Result<String, ApiError> {
    debug!("get_alarms: START");

    // First, read alarm/info count register (65001)
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((ParameterOperation::Read(CTC_ALARM_INFO_COUNT), response_tx))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    let (alarm_count, info_count) =
        match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx)
            .await
        {
            Ok(Ok(Ok(ModbusResponse::Value(value)))) => {
                #[allow(clippy::cast_possible_truncation)]
                #[allow(clippy::cast_sign_loss)]
                let raw_value = value as u16;
                let alarm_count = (raw_value & 0xFF) as u8;
                let info_count = ((raw_value >> 8) & 0xFF) as u8;
                debug!(
                    "get_alarms: Count read - alarms={}, infos={}",
                    alarm_count, info_count
                );
                (alarm_count, info_count)
            }
            Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
                error!("get_alarms: Unexpected RawRegisters response for count");
                return Err(ApiError::InternalError);
            }
            Ok(Ok(Err(e))) => {
                error!("get_alarms: Modbus error reading count - {}", e);
                return Err(ApiError::from(e));
            }
            Ok(Err(e)) => {
                error!("get_alarms: Failed to receive count response - {}", e);
                return Err(ApiError::ServiceUnavailable);
            }
            Err(_) => {
                error!(
                    "get_alarms: Timeout reading count after {}s",
                    state.request_timeout_secs
                );
                return Err(ApiError::Timeout);
            }
        };

    // If no alarms or infos, return early
    if alarm_count == 0 && info_count == 0 {
        debug!("get_alarms: No active alarms or infos");
        let response = AlarmResponse {
            alarm_count,
            info_count,
            alarms: Vec::new(),
            infos: Vec::new(),
        };
        return serde_json::to_string(&response)
            .map(|s| s + "\n")
            .map_err(|_| ApiError::InternalError);
    }

    // Read alarm bitmask and scan for active indices
    let active_alarm_refs = if alarm_count > 0 {
        debug!("get_alarms: Reading alarm bitmask registers");
        match read_raw_registers(&state, ALARM_BITMASK_START, ALARM_BITMASK_COUNT).await {
            Ok(bitmask) => {
                let refs = scan_bitmask(&bitmask);
                debug!(
                    "get_alarms: Found {} active alarm references: {:?}",
                    refs.len(),
                    refs
                );
                refs
            }
            Err(e) => {
                warn!("get_alarms: Failed to read alarm bitmask: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Read info bitmask and scan for active indices
    let active_info_refs = if info_count > 0 {
        debug!("get_alarms: Reading info bitmask registers");
        match read_raw_registers(&state, INFO_BITMASK_START, INFO_BITMASK_COUNT).await {
            Ok(bitmask) => {
                let refs = scan_bitmask(&bitmask);
                debug!(
                    "get_alarms: Found {} active info references: {:?}",
                    refs.len(),
                    refs
                );
                refs
            }
            Err(e) => {
                warn!("get_alarms: Failed to read info bitmask: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Fetch text for each ACTUALLY active alarm
    let mut alarms = Vec::new();
    let mut active_codes = HashSet::new();
    for reference in active_alarm_refs {
        match get_or_fetch_alarm_text(&state, reference, false).await {
            Ok(cached) => {
                let message_en = cached
                    .code
                    .as_ref()
                    .and_then(|c| ALARM_TRANSLATIONS.get(c.as_str()).map(|s| (*s).to_string()));
                let description = cached
                    .code
                    .as_ref()
                    .and_then(|c| ALARM_DESCRIPTIONS.get(c.as_str()).map(|s| (*s).to_string()));
                let description_sv = cached.code.as_ref().and_then(|c| {
                    ALARM_DESCRIPTIONS_SV
                        .get(c.as_str())
                        .map(|s| (*s).to_string())
                });
                // Track first-seen timestamp using code or reference as key
                let key = cached
                    .code
                    .clone()
                    .unwrap_or_else(|| format!("E{reference}"));
                active_codes.insert(key.clone());
                let first_seen_time = record_first_seen(&key);
                alarms.push(AlarmMessage {
                    reference,
                    code: cached.code,
                    message: cached.message,
                    message_en,
                    description,
                    description_sv,
                    first_seen: format_timestamp(first_seen_time),
                });
            }
            Err(e) => {
                warn!(
                    "get_alarms: Failed to fetch alarm {} text: {}",
                    reference, e
                );
                // Continue with remaining alarms
            }
        }
    }

    // Fetch text for each ACTUALLY active info
    let mut infos = Vec::new();
    for reference in active_info_refs {
        // Add 10000 offset for info references (as per CTC protocol)
        match get_or_fetch_alarm_text(&state, 10000 + reference, true).await {
            Ok(cached) => {
                let message_en = cached
                    .code
                    .as_ref()
                    .and_then(|c| ALARM_TRANSLATIONS.get(c.as_str()).map(|s| (*s).to_string()));
                let description = cached
                    .code
                    .as_ref()
                    .and_then(|c| ALARM_DESCRIPTIONS.get(c.as_str()).map(|s| (*s).to_string()));
                let description_sv = cached.code.as_ref().and_then(|c| {
                    ALARM_DESCRIPTIONS_SV
                        .get(c.as_str())
                        .map(|s| (*s).to_string())
                });
                // Track first-seen timestamp using code or reference as key
                let key = cached
                    .code
                    .clone()
                    .unwrap_or_else(|| format!("I{reference}"));
                active_codes.insert(key.clone());
                let first_seen_time = record_first_seen(&key);
                infos.push(AlarmMessage {
                    reference, // Return actual info number (e.g., 14), not offset
                    code: cached.code,
                    message: cached.message,
                    message_en,
                    description,
                    description_sv,
                    first_seen: format_timestamp(first_seen_time),
                });
            }
            Err(e) => {
                warn!("get_alarms: Failed to fetch info {} text: {}", reference, e);
                // Continue with remaining infos
            }
        }
    }

    // Clean up first-seen entries for alarms/infos that are no longer active
    cleanup_inactive_codes(&active_codes);

    debug!(
        "get_alarms: SUCCESS - {} alarms, {} infos fetched",
        alarms.len(),
        infos.len()
    );

    let response = AlarmResponse {
        alarm_count,
        info_count,
        alarms,
        infos,
    };

    serde_json::to_string(&response)
        .map(|s| s + "\n")
        .map_err(|_| ApiError::InternalError)
}

/// Get cached alarm text or fetch from device
#[allow(clippy::too_many_lines)]
async fn get_or_fetch_alarm_text(
    state: &AlarmState,
    reference: u16,
    is_info: bool,
) -> Result<CachedAlarmText, ApiError> {
    // Check cache first
    if let Ok(cache) = ALARM_TEXT_CACHE.read()
        && let Some(cached) = cache.get(&reference)
    {
        debug!(
            "get_or_fetch_alarm_text: Cache hit for reference {}",
            reference
        );
        return Ok(cached.clone());
    }

    debug!(
        "get_or_fetch_alarm_text: Cache miss for reference {}, fetching from device",
        reference
    );

    // Not cached - read from device
    // Step 1: Write reference to text buffer transfer register (65100)
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((
            ParameterOperation::WriteRawRegister {
                register: TEXT_BUFFER_TRANSFER_REG,
                value: reference,
            },
            response_tx,
        ))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(_))) => {
            debug!(
                "get_or_fetch_alarm_text: Write to text buffer transfer OK for ref {}",
                reference
            );
        }
        Ok(Ok(Err(e))) => {
            error!(
                "get_or_fetch_alarm_text: Failed to write transfer register - {}",
                e
            );
            return Err(ApiError::from(e));
        }
        Ok(Err(e)) => {
            error!(
                "get_or_fetch_alarm_text: Failed to receive write response - {}",
                e
            );
            return Err(ApiError::ServiceUnavailable);
        }
        Err(_) => {
            error!("get_or_fetch_alarm_text: Timeout writing transfer register");
            return Err(ApiError::Timeout);
        }
    }

    // Small delay to let CTC populate the buffer
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Step 2: Read text buffer (65101-65125, 25 registers)
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((
            ParameterOperation::ReadRawRegisters {
                start: TEXT_BUFFER_START_REG,
                count: TEXT_BUFFER_COUNT,
            },
            response_tx,
        ))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::RawRegisters { values, .. }))) => {
            let raw_text = decode_text_buffer(&values);
            debug!(
                "get_or_fetch_alarm_text: Read text buffer for ref {}: '{}'",
                reference, raw_text
            );

            let (code, message) = parse_alarm_text(&raw_text, is_info);

            let cached = CachedAlarmText {
                code,
                message,
                raw_text,
            };

            // Store in cache
            if let Ok(mut cache) = ALARM_TEXT_CACHE.write() {
                cache.insert(reference, cached.clone());
            }

            Ok(cached)
        }
        Ok(Ok(Ok(ModbusResponse::Value(_)))) => {
            error!("get_or_fetch_alarm_text: Unexpected Value response for text buffer");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!(
                "get_or_fetch_alarm_text: Failed to read text buffer - {}",
                e
            );
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!(
                "get_or_fetch_alarm_text: Failed to receive read response - {}",
                e
            );
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!("get_or_fetch_alarm_text: Timeout reading text buffer");
            Err(ApiError::Timeout)
        }
    }
}

/// Read raw registers from the device
async fn read_raw_registers(
    state: &AlarmState,
    start: u16,
    count: u16,
) -> Result<Vec<u16>, ApiError> {
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

    state
        .sender
        .send((
            ParameterOperation::ReadRawRegisters { start, count },
            response_tx,
        ))
        .await
        .map_err(|_| ApiError::ServiceUnavailable)?;

    match tokio::time::timeout(Duration::from_secs(state.request_timeout_secs), response_rx).await {
        Ok(Ok(Ok(ModbusResponse::RawRegisters { values, .. }))) => Ok(values),
        Ok(Ok(Ok(ModbusResponse::Value(_)))) => {
            error!("read_raw_registers: Unexpected Value response");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Err(e))) => {
            error!("read_raw_registers: Modbus error - {}", e);
            Err(ApiError::from(e))
        }
        Ok(Err(e)) => {
            error!("read_raw_registers: Failed to receive response - {}", e);
            Err(ApiError::ServiceUnavailable)
        }
        Err(_) => {
            error!(
                "read_raw_registers: Timeout after {}s",
                state.request_timeout_secs
            );
            Err(ApiError::Timeout)
        }
    }
}

/// Format a `SystemTime` as ISO 8601 string (e.g., "2025-01-15T14:30:00Z")
fn format_timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ModbusError;
    use crate::modbus::actor::ModbusResult;
    use tokio::sync::mpsc;

    type MockReceiver = mpsc::Receiver<(
        ParameterOperation,
        tokio::sync::oneshot::Sender<ModbusResult>,
    )>;

    fn create_mock_state() -> (AlarmState, MockReceiver) {
        let (tx, rx) = mpsc::channel(10);
        let state = AlarmState {
            sender: tx,
            request_timeout_secs: 5,
        };
        (state, rx)
    }

    #[test]
    fn test_decode_text_buffer() {
        // Test with "[E040] Low" encoded as low-byte first, high-byte second
        // '[' = 0x5B, 'E' = 0x45 -> register = 0x455B (low byte first)
        // '0' = 0x30, '4' = 0x34 -> register = 0x3430
        // '0' = 0x30, ']' = 0x5D -> register = 0x5D30
        // ' ' = 0x20, 'L' = 0x4C -> register = 0x4C20
        // 'o' = 0x6F, 'w' = 0x77 -> register = 0x776F
        let registers = vec![0x455B, 0x3430, 0x5D30, 0x4C20, 0x776F];
        let decoded = decode_text_buffer(&registers);
        assert_eq!(decoded, "[E040] Low");
    }

    #[test]
    fn test_decode_text_buffer_with_nulls() {
        // "TEST" followed by null padding (low-byte first, high-byte second)
        // 'T' = 0x54, 'E' = 0x45 -> register = 0x4554
        // 'S' = 0x53, 'T' = 0x54 -> register = 0x5453
        let registers = vec![0x4554, 0x5453, 0x0000, 0x0000];
        let decoded = decode_text_buffer(&registers);
        assert_eq!(decoded, "TEST");
    }

    #[test]
    fn test_parse_alarm_text_with_code() {
        let (code, message) = parse_alarm_text("[E040] Low brine flow", false);
        assert_eq!(code, Some("E040".to_string()));
        assert_eq!(message, "Low brine flow");
    }

    #[test]
    fn test_parse_alarm_text_without_code() {
        let (code, message) = parse_alarm_text("Some message without code", false);
        assert_eq!(code, None);
        assert_eq!(message, "Some message without code");
    }

    #[test]
    fn test_alarm_translations() {
        assert_eq!(ALARM_TRANSLATIONS.get("E040"), Some(&"Low brine flow"));
        assert_eq!(ALARM_TRANSLATIONS.get("I019"), Some(&"Smart: Low price"));
        assert_eq!(ALARM_TRANSLATIONS.get("XXXX"), None);
    }

    #[test]
    fn test_alarm_descriptions() {
        assert_eq!(
            ALARM_DESCRIPTIONS.get("I017"),
            Some(&"SmartGrid blocking mode - heating reduced due to grid signal or high prices")
        );
        assert_eq!(
            ALARM_DESCRIPTIONS.get("E040"),
            Some(&"Insufficient brine flow - check circulation pump and pipes for blockage")
        );
        assert_eq!(ALARM_DESCRIPTIONS.get("XXXX"), None);
    }

    #[test]
    fn test_alarm_descriptions_sv() {
        assert_eq!(
            ALARM_DESCRIPTIONS_SV.get("I017"),
            Some(&"SmartGrid blockeringsläge - värme reducerad pga nätsignal eller högt pris")
        );
        assert_eq!(
            ALARM_DESCRIPTIONS_SV.get("E040"),
            Some(&"Otillräckligt köldbärarflöde - kontrollera pump och rör")
        );
        assert_eq!(ALARM_DESCRIPTIONS_SV.get("XXXX"), None);
    }

    #[test]
    fn test_scan_bitmask_empty() {
        let registers = vec![0x0000, 0x0000];
        let active = scan_bitmask(&registers);
        assert!(active.is_empty());
    }

    #[test]
    fn test_scan_bitmask_single_bit() {
        // Bit 14 set in first register (info #14)
        let registers = vec![0x4000, 0x0000]; // 0x4000 = bit 14 set
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![14]);
    }

    #[test]
    fn test_scan_bitmask_multiple_bits() {
        // Bits 0, 5, and 14 set in first register
        let registers = vec![0x4021, 0x0000]; // 0x4021 = bits 0, 5, 14 set
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![0, 5, 14]);
    }

    #[test]
    fn test_scan_bitmask_across_registers() {
        // Bit 14 in reg 0 (index 14), bit 0 in reg 1 (index 16)
        let registers = vec![0x4000, 0x0001];
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![14, 16]);
    }

    #[tokio::test]
    async fn test_get_alarm_status_no_alarms() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarm_status(State(state)).await });

        // Respond with count = 0 (no alarms, no infos)
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx.send(Ok(ModbusResponse::Value(0.0))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"alarm_count\":0"));
        assert!(json.contains("\"info_count\":0"));
        assert!(json.contains("\"has_alarms\":false"));
        assert!(json.contains("\"has_infos\":false"));
    }

    #[tokio::test]
    async fn test_get_alarm_status_with_alarms() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarm_status(State(state)).await });

        // Respond with 2 alarms and 3 infos
        // Count = (3 << 8) | 2 = 0x0302 = 770
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx.send(Ok(ModbusResponse::Value(770.0))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"alarm_count\":2"));
        assert!(json.contains("\"info_count\":3"));
        assert!(json.contains("\"has_alarms\":true"));
        assert!(json.contains("\"has_infos\":true"));
    }

    #[tokio::test]
    async fn test_get_alarm_status_channel_closed() {
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        let result = get_alarm_status(State(state)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    #[tokio::test]
    async fn test_get_alarm_status_modbus_error() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarm_status(State(state)).await });

        // Respond with error
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Err(ModbusError::Timeout {
                    register: 65001,
                    operation: "test".to_string(),
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_get_alarms_no_active() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarms(State(state)).await });

        // Respond with count = 0
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx.send(Ok(ModbusResponse::Value(0.0))).unwrap();
        }

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"alarm_count\":0"));
        assert!(json.contains("\"info_count\":0"));
        assert!(json.contains("\"alarms\":[]"));
        assert!(json.contains("\"infos\":[]"));
    }

    #[tokio::test]
    async fn test_get_alarms_channel_closed() {
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        let result = get_alarms(State(state)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }
}
