//! Alarm and info message API endpoints
//!
//! These endpoints provide access to active alarms and info messages from the CTC system.
//! The system uses a two-tier approach:
//! - Quick status check (for polling): reads just the count register
//! - Full details: reads bitmasks and text buffers for active alarms/info

use std::collections::{HashMap, HashSet};
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
use crate::modbus::bms_parameters::{CTC_ALARM_INFO_COUNT, INFO_REF_OFFSET};
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

/// Serialises concurrent `get_alarms` requests so two callers cannot race on
/// building `active_codes` and calling `cleanup_inactive_codes` against each
/// other's view of the bitmask.
static ALARMS_HANDLER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Alarm bitmask: 50 registers starting at 65010 (800 alarms max)
const ALARM_BITMASK_START: u16 = 65010;
const ALARM_BITMASK_COUNT: u16 = 50;

/// Encode an info index as a text buffer transfer reference
fn info_ref(idx: u16) -> u16 {
    INFO_REF_OFFSET + idx
}

/// Namespace tag for first-seen cache keys. Alarms and infos can share the
/// same code value (e.g. "117"), so the persisted key prefix keeps them in
/// separate buckets in `ALARM_FIRST_SEEN`.
#[derive(Copy, Clone, Debug)]
enum AlarmKind {
    Alarm,
    Info,
}

impl AlarmKind {
    fn cache_key(self, code: impl std::fmt::Display) -> String {
        let prefix = match self {
            Self::Alarm => 'A',
            Self::Info => 'I',
        };
        format!("{prefix}:{code}")
    }
}

/// Decode a text buffer transfer reference back to an info index, if it is one
#[allow(dead_code)]
fn decode_info_ref(reference: u16) -> Option<u16> {
    reference.checked_sub(INFO_REF_OFFSET)
}

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

/// Full alarm response with all active alarms and infos.
///
/// `alarm_count` / `info_count` match the lengths of the returned vectors,
/// which can be smaller than the device-reported count if some text fetches
/// failed. `partial = true` signals that case so dashboards can show a
/// "fetch failed" hint instead of silently under-reporting.
#[derive(Serialize)]
struct AlarmResponse {
    alarm_count: u8,
    info_count: u8,
    alarms: Vec<AlarmMessage>,
    infos: Vec<AlarmMessage>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    partial: bool,
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
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("get_alarm_status: Unexpected Stats response");
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
    // Serialise concurrent requests so two callers cannot observe slightly
    // different bitmask states and have one's cleanup evict entries the other
    // still considers active.
    let _guard = ALARMS_HANDLER_LOCK.lock().await;

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
            Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
                error!("get_alarms: Unexpected Stats response for count");
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

    // If no alarms or infos, return early — but first reap any stale
    // ALARM_FIRST_SEEN entries from a previously-active code. The device
    // reporting zero counts IS authoritative evidence that nothing is active,
    // so an empty active_codes set is the right thing to retain against.
    if alarm_count == 0 && info_count == 0 {
        debug!("get_alarms: No active alarms or infos");
        cleanup_inactive_codes(&HashSet::new());
        let response = AlarmResponse {
            alarm_count,
            info_count,
            alarms: Vec::new(),
            infos: Vec::new(),
            partial: false,
        };
        return serde_json::to_string(&response)
            .map(|s| s + "\n")
            .map_err(|_| ApiError::InternalError);
    }

    // Read alarm bitmask and scan for active indices.
    // Track whether the read succeeded: on failure we cannot tell which codes
    // are still active, so we must not evict any first-seen entries.
    let mut alarm_bitmask_ok = true;
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
                alarm_bitmask_ok = false;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Read info bitmask and scan for active indices.
    let mut info_bitmask_ok = true;
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
                info_bitmask_ok = false;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // Fetch text for each ACTUALLY active alarm
    let mut alarms = Vec::new();
    let mut active_codes = HashSet::new();
    // Track whether every text-fetch succeeded. A failure means `active_codes`
    // is missing the code-based key for that reference, so cleanup would
    // orphan its `ALARM_FIRST_SEEN["A:{code}"]` entry. The reference-keyed
    // fallback that used to be inserted here did not match the success path's
    // code-keyed entries either, so cleanup evicted them anyway.
    let mut all_text_fetches_ok = true;
    for reference in active_alarm_refs {
        match get_or_fetch_alarm_text(&state, reference).await {
            Ok(cached) => {
                let message_en = lookup_translation(cached.code.as_deref());
                let description = lookup_description(cached.code.as_deref());
                let description_sv = lookup_description_sv(cached.code.as_deref());
                // Track first-seen timestamp using code or reference as key.
                // Namespace with "A:" so alarms and infos that share a code
                // value (e.g. "117") cannot shadow each other in the map.
                let key = cached.code.as_ref().map_or_else(
                    || AlarmKind::Alarm.cache_key(reference),
                    |c| AlarmKind::Alarm.cache_key(c),
                );
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
                all_text_fetches_ok = false;
                // Continue with remaining alarms
            }
        }
    }

    // Fetch text for each ACTUALLY active info
    let mut infos = Vec::new();
    for reference in active_info_refs {
        // Add info offset for info references (as per CTC protocol)
        match get_or_fetch_alarm_text(&state, info_ref(reference)).await {
            Ok(cached) => {
                let message_en = lookup_translation(cached.code.as_deref());
                let description = lookup_description(cached.code.as_deref());
                let description_sv = lookup_description_sv(cached.code.as_deref());
                // Track first-seen timestamp using code or reference as key.
                // Namespace with "I:" so infos and alarms that share a code
                // value (e.g. "117") cannot shadow each other in the map.
                let key = cached.code.as_ref().map_or_else(
                    || AlarmKind::Info.cache_key(reference),
                    |c| AlarmKind::Info.cache_key(c),
                );
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
                all_text_fetches_ok = false;
                // Continue with remaining infos
            }
        }
    }

    // Clean up first-seen entries for alarms/infos that are no longer active.
    // Only safe to evict when bitmask reads AND every text fetch succeeded —
    // otherwise `active_codes` is incomplete and we would drop still-active
    // entries. Note: ALARM_FIRST_SEEN is in-memory only and does not persist
    // across restarts.
    if alarm_bitmask_ok && info_bitmask_ok && all_text_fetches_ok {
        cleanup_inactive_codes(&active_codes);
    } else {
        debug!(
            "get_alarms: Skipping cleanup_inactive_codes due to transient read failure (bitmask alarm={alarm_bitmask_ok} info={info_bitmask_ok} text={all_text_fetches_ok})"
        );
    }

    debug!(
        "get_alarms: SUCCESS - reported {} of {} active alarms, {} of {} active infos",
        alarms.len(),
        alarm_count,
        infos.len(),
        info_count,
    );

    // Report counts that match the returned vectors so clients see a
    // consistent view even when some text fetches failed or the bitmask
    // read returned no bits. `partial` flags the case where at least one
    // read failed — clients can show a "fetch failed" hint instead of
    // silently treating an under-report as the truth.
    let partial = !(alarm_bitmask_ok && info_bitmask_ok && all_text_fetches_ok);
    #[allow(clippy::cast_possible_truncation)]
    let response = AlarmResponse {
        alarm_count: alarms.len().min(u8::MAX as usize) as u8,
        info_count: infos.len().min(u8::MAX as usize) as u8,
        alarms,
        infos,
        partial,
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
) -> Result<CachedAlarmText, ApiError> {
    // Check cache first. Recover from poisoning — a previous panic left the
    // map in an indeterminate-but-usable state, and falling through to a fresh
    // device fetch on every request defeats the cache entirely.
    {
        let cache = ALARM_TEXT_CACHE
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = cache.get(&reference) {
            debug!(
                "get_or_fetch_alarm_text: Cache hit for reference {}",
                reference
            );
            return Ok(cached.clone());
        }
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

            let (code, message) = parse_alarm_text(&raw_text);

            let cached = CachedAlarmText {
                code,
                message,
                raw_text,
            };

            // Store in cache. Recover from poisoning to match the read path.
            {
                let mut cache = ALARM_TEXT_CACHE
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                cache.insert(reference, cached.clone());
            }

            Ok(cached)
        }
        Ok(Ok(Ok(ModbusResponse::Value(_)))) => {
            error!("get_or_fetch_alarm_text: Unexpected Value response for text buffer");
            Err(ApiError::InternalError)
        }
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("get_or_fetch_alarm_text: Unexpected Stats response for text buffer");
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
        Ok(Ok(Ok(ModbusResponse::Stats(_)))) => {
            error!("read_raw_registers: Unexpected Stats response");
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

fn lookup(code: Option<&str>, map: &HashMap<&str, &str>, what: &str) -> Option<String> {
    let code = code?;
    if let Some(s) = map.get(code) {
        Some((*s).to_string())
    } else {
        debug!("Alarm code {code} has no {what}");
        None
    }
}

fn lookup_translation(code: Option<&str>) -> Option<String> {
    lookup(code, &ALARM_TRANSLATIONS, "English translation")
}
fn lookup_description(code: Option<&str>) -> Option<String> {
    lookup(code, &ALARM_DESCRIPTIONS, "English description")
}
fn lookup_description_sv(code: Option<&str>) -> Option<String> {
    lookup(code, &ALARM_DESCRIPTIONS_SV, "Swedish description")
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

    /// State with a zero-second request timeout so a handler awaiting a reply
    /// that never arrives hits the `Err(Elapsed)` arm immediately.
    fn create_mock_state_timeout() -> (AlarmState, MockReceiver) {
        let (tx, rx) = mpsc::channel(10);
        let state = AlarmState {
            sender: tx,
            request_timeout_secs: 0,
        };
        (state, rx)
    }

    /// Encode a string into a low-byte-first text-buffer register vector, the
    /// same layout `decode_text_buffer` expects (two chars per register, low
    /// byte first). Pads to `TEXT_BUFFER_COUNT` registers with zeros.
    fn encode_text_buffer(s: &str) -> Vec<u16> {
        let bytes = s.as_bytes();
        let mut regs = vec![0u16; usize::from(TEXT_BUFFER_COUNT)];
        for (i, chunk) in bytes.chunks(2).enumerate() {
            if i >= regs.len() {
                break;
            }
            let lo = u16::from(chunk[0]);
            let hi = chunk.get(1).map_or(0, |&b| u16::from(b));
            regs[i] = (hi << 8) | lo;
        }
        regs
    }

    /// Remove specific references from the global alarm-text cache so a test
    /// can deterministically force a cache miss (device fetch) for them.
    fn evict_cache(refs: &[u16]) {
        let mut cache = ALARM_TEXT_CACHE
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for r in refs {
            cache.remove(r);
        }
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
        let (code, message) = parse_alarm_text("[E040] Low brine flow");
        assert_eq!(code, Some("E040".to_string()));
        assert_eq!(message, "Low brine flow");
    }

    #[test]
    fn test_parse_alarm_text_without_code() {
        let (code, message) = parse_alarm_text("Some message without code");
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
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
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
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        let (state, rx) = create_mock_state();

        // Drop receiver to simulate actor shutdown
        drop(rx);

        let result = get_alarms(State(state)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::ServiceUnavailable));
    }

    /// Two concurrent `get_alarms` calls must be serialised by
    /// `ALARMS_HANDLER_LOCK`: while one call is between its bitmask scan and
    /// its `cleanup_inactive_codes`, the other must not have begun. Otherwise
    /// the second call's cleanup could evict first-seen entries the first
    /// call is mid-way through recording.
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        clippy::items_after_statements,
        clippy::similar_names,
        clippy::cast_sign_loss
    )]
    async fn test_get_alarms_concurrent_calls_are_serialised() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        use crate::messages::types::ALARM_FIRST_SEEN;
        // The guard above clears ALARM_FIRST_SEEN before this test and serialises
        // it against other alarm tests so no leaked entries skew the assertions.
        const REF_A: u16 = 700;
        const REF_B: u16 = 701;
        let key_a = AlarmKind::Alarm.cache_key(format!("E{REF_A}"));
        let key_b = AlarmKind::Alarm.cache_key(format!("E{REF_B}"));
        let stale_key = AlarmKind::Alarm.cache_key("STALE_FOR_SERIALISATION_TEST");

        // Pre-populate the alarm text cache so `get_or_fetch_alarm_text`
        // returns synchronously — avoids needing to mock the 200 ms
        // transfer-register handshake per active alarm.
        {
            let mut cache = ALARM_TEXT_CACHE
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.insert(
                REF_A,
                CachedAlarmText {
                    code: Some(format!("E{REF_A}")),
                    message: "Test alarm A".to_string(),
                    raw_text: format!("[E{REF_A}] Test alarm A"),
                },
            );
            cache.insert(
                REF_B,
                CachedAlarmText {
                    code: Some(format!("E{REF_B}")),
                    message: "Test alarm B".to_string(),
                    raw_text: format!("[E{REF_B}] Test alarm B"),
                },
            );
        }
        // Seed a stale first-seen entry. Neither caller's active set will
        // contain it, so `cleanup_inactive_codes` must evict it. Recover
        // from poison so a panicking sibling test doesn't block this one.
        {
            let mut first = ALARM_FIRST_SEEN
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            first.insert(stale_key.clone(), SystemTime::now());
            first.remove(&key_a);
            first.remove(&key_b);
        }

        // Build the bitmask response with bits set for REF_A and REF_B.
        let mut alarm_bitmask = vec![0u16; usize::from(ALARM_BITMASK_COUNT)];
        for r in [REF_A, REF_B] {
            let reg = usize::from(r / 16);
            let bit = r % 16;
            alarm_bitmask[reg] |= 1 << bit;
        }

        // Two callers, each with their own mock channel so we can observe
        // which call is currently issuing requests.
        let (state_a, mut rx_a) = create_mock_state();
        let (state_b, mut rx_b) = create_mock_state();
        let bitmask_a = alarm_bitmask.clone();
        let bitmask_b = alarm_bitmask.clone();

        async fn serve_get_alarms(rx: &mut MockReceiver, bitmask: Vec<u16>) {
            // 1. Count read: 2 alarms, 0 infos => 0x0002.
            let (op, tx) = rx.recv().await.expect("count read");
            assert!(matches!(op, ParameterOperation::Read(_)));
            tx.send(Ok(ModbusResponse::Value(2.0))).unwrap();
            // 2. Alarm bitmask read.
            let (op, tx) = rx.recv().await.expect("alarm bitmask read");
            match op {
                ParameterOperation::ReadRawRegisters { start, count } => {
                    assert_eq!(start, ALARM_BITMASK_START);
                    assert_eq!(count, ALARM_BITMASK_COUNT);
                }
                _ => panic!("expected ReadRawRegisters, got {op:?}"),
            }
            tx.send(Ok(ModbusResponse::RawRegisters {
                start: ALARM_BITMASK_START,
                values: bitmask,
            }))
            .unwrap();
            // No info bitmask read because info_count == 0.
            // No text-buffer transfers because we primed ALARM_TEXT_CACHE.
        }

        let h_a = tokio::spawn(async move { get_alarms(State(state_a)).await });
        let h_b = tokio::spawn(async move { get_alarms(State(state_b)).await });

        // Whichever caller wins the lock first issues its count read first;
        // identify it by selecting on the two mock channels.
        let first_is_a = tokio::select! {
            biased;
            req = rx_a.recv() => {
                let (op, tx) = req.expect("first count read");
                assert!(matches!(op, ParameterOperation::Read(_)));
                tx.send(Ok(ModbusResponse::Value(2.0))).unwrap();
                true
            }
            req = rx_b.recv() => {
                let (op, tx) = req.expect("first count read");
                assert!(matches!(op, ParameterOperation::Read(_)));
                tx.send(Ok(ModbusResponse::Value(2.0))).unwrap();
                false
            }
        };

        let (first_rx, second_rx, first_bitmask, second_bitmask) = if first_is_a {
            (&mut rx_a, &mut rx_b, bitmask_a, bitmask_b)
        } else {
            (&mut rx_b, &mut rx_a, bitmask_b, bitmask_a)
        };

        // Drive the runtime so the second caller's task is definitely polled
        // and has reached the handler mutex. Without these yields, "no message
        // on second_rx" might mean "task never ran" rather than "task parked
        // on the mutex" — which would make the probe assert nothing useful.
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }

        // With the runtime driven, the second caller has been polled and is
        // parked on the handler mutex. Its mock channel must stay silent until
        // the first caller releases the lock. try_recv is deterministic and
        // doesn't rely on a wall-clock probe.
        match second_rx.try_recv() {
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {} // expected: blocked
            Ok((op, _tx)) => panic!(
                "second caller was not blocked on the handler mutex — \
                 received {op:?} while first caller is still inside get_alarms"
            ),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                panic!("second mock channel disconnected unexpectedly")
            }
        }

        let (op, tx) = first_rx.recv().await.expect("first alarm bitmask read");
        match op {
            ParameterOperation::ReadRawRegisters { start, count } => {
                assert_eq!(start, ALARM_BITMASK_START);
                assert_eq!(count, ALARM_BITMASK_COUNT);
            }
            other => panic!("expected ReadRawRegisters, got {other:?}"),
        }
        tx.send(Ok(ModbusResponse::RawRegisters {
            start: ALARM_BITMASK_START,
            values: first_bitmask,
        }))
        .unwrap();

        // First caller finishes (cache served the text). Now the second
        // caller's count read becomes available.
        serve_get_alarms(second_rx, second_bitmask).await;

        let r_a = h_a.await.unwrap();
        let r_b = h_b.await.unwrap();
        assert!(r_a.is_ok(), "caller A failed: {r_a:?}");
        assert!(r_b.is_ok(), "caller B failed: {r_b:?}");

        // After both completions, ALARM_FIRST_SEEN must contain both active
        // entries and the stale key must be evicted.
        let first = ALARM_FIRST_SEEN
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            first.contains_key(&key_a),
            "expected {key_a} to be recorded; have {:?}",
            first.keys().collect::<Vec<_>>()
        );
        assert!(
            first.contains_key(&key_b),
            "expected {key_b} to be recorded; have {:?}",
            first.keys().collect::<Vec<_>>()
        );
        assert!(
            !first.contains_key(&stale_key),
            "stale entry should have been evicted by cleanup_inactive_codes"
        );
    }

    // --- get_alarm_status: remaining response arms ---

    #[tokio::test]
    async fn test_get_alarm_status_unexpected_raw_registers() {
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarm_status(State(state)).await });

        // A scalar count read must reply with Value; RawRegisters is wrong.
        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Ok(ModbusResponse::RawRegisters {
                    start: 65001,
                    values: vec![1],
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_get_alarm_status_timeout() {
        let (state, mut rx) = create_mock_state_timeout();

        let handle = tokio::spawn(async move { get_alarm_status(State(state)).await });

        // Receive but never respond; the zero-second timeout elapses.
        let _held = rx.recv().await.map(|(_, tx)| tx);

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    // --- get_alarms: full text-fetch path via the shared fake actor ---

    /// One active alarm (E040). Drives the full handler: count -> alarm
    /// bitmask scan -> transfer-register write -> text-buffer read -> decode
    /// -> translation/description lookup. Asserts the decoded code, the raw
    /// message, and that the English/Swedish lookups resolve from the catalog.
    #[tokio::test]
    async fn test_get_alarms_one_alarm_full_fetch() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        // Force a device fetch for ref 40 so the transfer/read/decode path runs.
        evict_cache(&[40]);

        // Alarm ref 40 => bitmask register index 40/16 = 2 (addr 65012), bit 8.
        let mut reads = std::collections::HashMap::new();
        reads.insert(CTC_ALARM_INFO_COUNT.id, 1u16); // 1 alarm, 0 infos
        reads.insert(ALARM_BITMASK_START + 2, 1u16 << 8); // bit 8 of reg 2 => ref 40
        // Seed the text buffer with the device text for E040.
        for (i, &v) in encode_text_buffer("[E040] Low brine flow")
            .iter()
            .enumerate()
        {
            reads.insert(TEXT_BUFFER_START_REG + u16::try_from(i).unwrap(), v);
        }

        let sender = crate::modbus::test_support::spawn_fake_actor(reads);
        let state = AlarmState {
            sender,
            request_timeout_secs: 5,
        };

        let json = get_alarms(State(state)).await.expect("get_alarms");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        assert_eq!(v["alarm_count"].as_u64(), Some(1));
        assert_eq!(v["info_count"].as_u64(), Some(0));
        // partial is omitted (false) when all reads succeed.
        assert!(v.get("partial").is_none());

        let alarms = v["alarms"].as_array().expect("alarms array");
        assert_eq!(alarms.len(), 1);
        let a = &alarms[0];
        assert_eq!(a["reference"].as_u64(), Some(40));
        assert_eq!(a["code"].as_str(), Some("E040"));
        assert_eq!(a["message"].as_str(), Some("Low brine flow"));
        // Catalog lookups resolve for E040.
        assert_eq!(a["message_en"].as_str(), Some("Low brine flow"));
        assert_eq!(
            a["description"].as_str(),
            Some("Insufficient brine flow - check circulation pump and pipes for blockage")
        );
        assert_eq!(
            a["description_sv"].as_str(),
            Some("Otillräckligt köldbärarflöde - kontrollera pump och rör")
        );
        assert!(a["first_seen"].is_string());
    }

    /// Cache-hit path: prime the global cache for the active reference, then
    /// drive `get_alarms` with a fake actor whose text-buffer registers are
    /// EMPTY. If the handler re-read the device it would decode an empty
    /// message; instead it serves the primed cache entry, proving the
    /// transfer-write / text-read round trip was skipped.
    #[tokio::test]
    async fn test_get_alarms_cache_hit_skips_device_read() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();

        let alarm_ref: u16 = 40;
        // Prime the cache with a sentinel message distinct from anything the
        // empty text buffer could decode to.
        {
            let mut cache = ALARM_TEXT_CACHE
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.insert(
                alarm_ref,
                CachedAlarmText {
                    code: Some("E040".to_string()),
                    message: "CACHED-SENTINEL".to_string(),
                    raw_text: "[E040] CACHED-SENTINEL".to_string(),
                },
            );
        }

        let mut reads = std::collections::HashMap::new();
        reads.insert(CTC_ALARM_INFO_COUNT.id, 1u16); // 1 alarm
        reads.insert(ALARM_BITMASK_START + 2, 1u16 << 8); // ref 40
        // Deliberately DO NOT seed text-buffer registers (all read as 0).

        let sender = crate::modbus::test_support::spawn_fake_actor(reads);
        let state = AlarmState {
            sender,
            request_timeout_secs: 5,
        };

        let json = get_alarms(State(state)).await.expect("get_alarms");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let a = &v["alarms"].as_array().expect("alarms array")[0];
        // The cached message wins; an empty-buffer re-read would NOT yield this.
        assert_eq!(a["message"].as_str(), Some("CACHED-SENTINEL"));
        assert_eq!(a["code"].as_str(), Some("E040"));
    }

    /// Active alarm AND active info, both served from primed cache. Confirms
    /// both vectors populate and the info reference is reported as the bare
    /// index (not the `INFO_REF_OFFSET`-adjusted value).
    #[tokio::test]
    async fn test_get_alarms_alarm_and_info() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();

        let alarm_ref: u16 = 40;
        let info_idx: u16 = 19;
        let info_reference = info_ref(info_idx);
        {
            let mut cache = ALARM_TEXT_CACHE
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.insert(
                alarm_ref,
                CachedAlarmText {
                    code: Some("E040".to_string()),
                    message: "Low brine flow".to_string(),
                    raw_text: "[E040] Low brine flow".to_string(),
                },
            );
            cache.insert(
                info_reference,
                CachedAlarmText {
                    code: Some("I019".to_string()),
                    message: "Smart: Low price".to_string(),
                    raw_text: "[I019] Smart: Low price".to_string(),
                },
            );
        }

        let mut reads = std::collections::HashMap::new();
        reads.insert(CTC_ALARM_INFO_COUNT.id, (1u16 << 8) | 1); // 1 alarm, 1 info
        reads.insert(ALARM_BITMASK_START + 2, 1u16 << 8); // alarm ref 40
        // Info idx 19 => reg index 19/16 = 1 (addr INFO_BITMASK_START+1), bit 3.
        reads.insert(INFO_BITMASK_START + 1, 1u16 << 3);

        let sender = crate::modbus::test_support::spawn_fake_actor(reads);
        let state = AlarmState {
            sender,
            request_timeout_secs: 5,
        };

        let json = get_alarms(State(state)).await.expect("get_alarms");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

        assert_eq!(v["alarm_count"].as_u64(), Some(1));
        assert_eq!(v["info_count"].as_u64(), Some(1));

        let info = &v["infos"].as_array().expect("infos array")[0];
        // Reported reference is the bare index, not info_ref(19).
        assert_eq!(info["reference"].as_u64(), Some(u64::from(info_idx)));
        assert_eq!(info["code"].as_str(), Some("I019"));
        assert_eq!(info["message_en"].as_str(), Some("Smart: Low price"));
    }

    /// Count reports an alarm, but the bitmask read fails. The handler must
    /// not error: it returns an empty alarm list with `partial = true`.
    #[tokio::test]
    async fn test_get_alarms_bitmask_read_failure_is_partial() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarms(State(state)).await });

        // 1. Count read: 1 alarm, 0 infos.
        let (op, tx) = rx.recv().await.expect("count read");
        assert!(matches!(op, ParameterOperation::Read(_)));
        tx.send(Ok(ModbusResponse::Value(1.0))).unwrap();

        // 2. Alarm bitmask read: reply with an error.
        let (op, tx) = rx.recv().await.expect("bitmask read");
        assert!(matches!(op, ParameterOperation::ReadRawRegisters { .. }));
        tx.send(Err(ModbusError::Timeout {
            register: ALARM_BITMASK_START,
            operation: "bitmask".to_string(),
        }))
        .unwrap();

        let json = handle.await.unwrap().expect("get_alarms");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["alarm_count"].as_u64(), Some(0));
        assert_eq!(v["alarms"].as_array().expect("alarms").len(), 0);
        assert_eq!(v["partial"].as_bool(), Some(true));
    }

    /// Bitmask scan finds an active alarm but its text fetch fails. The alarm
    /// is dropped from the list and `partial` is set.
    #[tokio::test]
    async fn test_get_alarms_text_fetch_failure_is_partial() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        evict_cache(&[40]);
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarms(State(state)).await });

        // 1. Count: 1 alarm.
        let (_, tx) = rx.recv().await.expect("count read");
        tx.send(Ok(ModbusResponse::Value(1.0))).unwrap();

        // 2. Alarm bitmask: set bit for ref 40.
        let (_, tx) = rx.recv().await.expect("bitmask read");
        let mut bitmask = vec![0u16; usize::from(ALARM_BITMASK_COUNT)];
        bitmask[2] = 1u16 << 8;
        tx.send(Ok(ModbusResponse::RawRegisters {
            start: ALARM_BITMASK_START,
            values: bitmask,
        }))
        .unwrap();

        // 3. Transfer-register write succeeds.
        let (op, tx) = rx.recv().await.expect("transfer write");
        assert!(matches!(op, ParameterOperation::WriteRawRegister { .. }));
        tx.send(Ok(ModbusResponse::Value(40.0))).unwrap();

        // 4. Text-buffer read fails.
        let (op, tx) = rx.recv().await.expect("text read");
        assert!(matches!(op, ParameterOperation::ReadRawRegisters { .. }));
        tx.send(Err(ModbusError::Timeout {
            register: TEXT_BUFFER_START_REG,
            operation: "text".to_string(),
        }))
        .unwrap();

        let json = handle.await.unwrap().expect("get_alarms");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["alarm_count"].as_u64(), Some(0));
        assert_eq!(v["partial"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn test_get_alarms_count_unexpected_response() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_alarms(State(state)).await });

        if let Some((ParameterOperation::Read(_), response_tx)) = rx.recv().await {
            response_tx
                .send(Ok(ModbusResponse::RawRegisters {
                    start: 65001,
                    values: vec![1],
                }))
                .unwrap();
        }

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_get_alarms_count_timeout() {
        let _guard = crate::messages::types::clear_alarm_first_seen_guard();
        let (state, mut rx) = create_mock_state_timeout();

        let handle = tokio::spawn(async move { get_alarms(State(state)).await });
        let _held = rx.recv().await.map(|(_, tx)| tx);

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    // --- get_or_fetch_alarm_text: error/unexpected arms ---

    #[tokio::test]
    async fn test_get_or_fetch_alarm_text_write_error() {
        evict_cache(&[5000]);
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_or_fetch_alarm_text(&state, 5000).await });

        // Transfer-register write fails.
        let (op, tx) = rx.recv().await.expect("transfer write");
        assert!(matches!(op, ParameterOperation::WriteRawRegister { .. }));
        tx.send(Err(ModbusError::Timeout {
            register: TEXT_BUFFER_TRANSFER_REG,
            operation: "transfer".to_string(),
        }))
        .unwrap();

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    #[tokio::test]
    async fn test_get_or_fetch_alarm_text_unexpected_value_on_read() {
        evict_cache(&[5001]);
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_or_fetch_alarm_text(&state, 5001).await });

        // Transfer write OK.
        let (_, tx) = rx.recv().await.expect("transfer write");
        tx.send(Ok(ModbusResponse::Value(5001.0))).unwrap();

        // Text-buffer read returns a scalar Value instead of RawRegisters.
        let (op, tx) = rx.recv().await.expect("text read");
        assert!(matches!(op, ParameterOperation::ReadRawRegisters { .. }));
        tx.send(Ok(ModbusResponse::Value(0.0))).unwrap();

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_get_or_fetch_alarm_text_decodes_and_caches() {
        evict_cache(&[5002]);
        let (state, mut rx) = create_mock_state();

        let handle = tokio::spawn(async move { get_or_fetch_alarm_text(&state, 5002).await });

        let (_, tx) = rx.recv().await.expect("transfer write");
        tx.send(Ok(ModbusResponse::Value(5002.0))).unwrap();

        let (_, tx) = rx.recv().await.expect("text read");
        tx.send(Ok(ModbusResponse::RawRegisters {
            start: TEXT_BUFFER_START_REG,
            values: encode_text_buffer("[E040] Low brine flow"),
        }))
        .unwrap();

        let cached = handle.await.unwrap().expect("cached text");
        assert_eq!(cached.code.as_deref(), Some("E040"));
        assert_eq!(cached.message, "Low brine flow");

        // The entry must now be in the global cache.
        let cache = ALARM_TEXT_CACHE
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            cache.get(&5002).map(|c| c.message.as_str()),
            Some("Low brine flow")
        );
    }

    // --- read_raw_registers: error/unexpected arms ---

    #[tokio::test]
    async fn test_read_raw_registers_unexpected_value() {
        let (state, mut rx) = create_mock_state();

        let handle =
            tokio::spawn(async move { read_raw_registers(&state, ALARM_BITMASK_START, 1).await });

        let (op, tx) = rx.recv().await.expect("raw read");
        assert!(matches!(op, ParameterOperation::ReadRawRegisters { .. }));
        tx.send(Ok(ModbusResponse::Value(1.0))).unwrap();

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::InternalError));
    }

    #[tokio::test]
    async fn test_read_raw_registers_timeout() {
        let (state, mut rx) = create_mock_state_timeout();

        let handle =
            tokio::spawn(async move { read_raw_registers(&state, ALARM_BITMASK_START, 1).await });
        let _held = rx.recv().await.map(|(_, tx)| tx);

        let result = handle.await.unwrap();
        assert!(matches!(result.unwrap_err(), ApiError::Timeout));
    }

    // --- lookup helpers: None and not-found branches ---

    #[test]
    fn test_lookup_none_code() {
        assert!(lookup_translation(None).is_none());
        assert!(lookup_description(None).is_none());
        assert!(lookup_description_sv(None).is_none());
    }

    #[test]
    fn test_lookup_unknown_code() {
        assert!(lookup_translation(Some("ZZZZ")).is_none());
        assert!(lookup_description(Some("ZZZZ")).is_none());
        assert!(lookup_description_sv(Some("ZZZZ")).is_none());
    }

    #[test]
    fn test_lookup_known_code() {
        assert_eq!(
            lookup_translation(Some("E040")).as_deref(),
            Some("Low brine flow")
        );
    }

    #[test]
    fn test_decode_info_ref_roundtrip() {
        assert_eq!(decode_info_ref(info_ref(19)), Some(19));
        // A reference below the info offset is not an info ref.
        assert_eq!(decode_info_ref(40), None);
    }
}
