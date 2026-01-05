//! Alarm and info message types, caches, and parsing utilities
//!
//! Contains data structures for alarm/info messages, memory caches for
//! efficient retrieval, and parsing functions for CTC text buffers.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

use nom::{
    IResult, Parser,
    bytes::complete::{tag, take_while1},
    sequence::delimited,
};
use serde::Serialize;

/// Memory cache for alarm reference -> parsed text mapping
/// Minimizes writes to register 65100 (text buffer transfer)
pub static ALARM_TEXT_CACHE: LazyLock<RwLock<HashMap<u16, CachedAlarmText>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Memory cache for alarm/info code -> first seen timestamp
/// Tracks when each alarm or info was first detected (survives page refreshes)
pub static ALARM_FIRST_SEEN: LazyLock<RwLock<HashMap<String, SystemTime>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Cached alarm text data
#[derive(Clone, Debug)]
pub struct CachedAlarmText {
    pub code: Option<String>,
    pub message: String,
    #[allow(dead_code)] // Kept for debugging and future use
    pub raw_text: String,
}

/// Individual alarm/info message for API response
#[derive(Serialize)]
pub struct AlarmMessage {
    pub reference: u16,
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_sv: Option<String>,
    /// Timestamp when this alarm/info was first seen (ISO 8601 format)
    pub first_seen: String,
}

/// Decode text buffer registers into a string
/// CTC uses Latin-1 encoding with low byte first, high byte second in each register
pub fn decode_text_buffer(registers: &[u16]) -> String {
    let mut bytes = Vec::new();
    for &reg in registers {
        let low = (reg & 0xFF) as u8;
        let high = ((reg >> 8) & 0xFF) as u8;
        // Add low byte first, then high byte
        if low > 31 && low != 127 {
            bytes.push(low);
        }
        if high > 31 && high != 127 {
            bytes.push(high);
        }
    }
    latin1_to_utf8(&bytes).trim().to_string()
}

/// Convert Latin-1 bytes to UTF-8 string
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Parse alarm code from text (e.g., "[E040]" or "[I019]")
pub fn parse_alarm_code(input: &str) -> IResult<&str, &str> {
    delimited(
        tag("["),
        take_while1(|c: char| c.is_alphanumeric()),
        tag("]"),
    )
    .parse(input)
}

/// Parse alarm text and extract code and message
pub fn parse_alarm_text(raw_text: &str, _is_info: bool) -> (Option<String>, String) {
    let trimmed = raw_text.trim();

    // Parse [E063] or [I019] from start of string
    match parse_alarm_code(trimmed) {
        Ok((rest, code)) => {
            let message = rest.trim().to_string();
            (Some(code.to_string()), message)
        }
        Err(_) => {
            // No code found, use entire text as message
            (None, trimmed.to_string())
        }
    }
}

/// Scan bitmask registers and return indices of set bits
/// Each register contains 16 bits, so index = `reg_idx` * 16 + `bit_position`
pub fn scan_bitmask(registers: &[u16]) -> Vec<u16> {
    let mut active = Vec::new();
    for (reg_idx, &value) in registers.iter().enumerate() {
        for bit in 0..16 {
            if (value >> bit) & 1 == 1 {
                #[allow(clippy::cast_possible_truncation)]
                let index = (reg_idx as u16) * 16 + bit;
                active.push(index);
            }
        }
    }
    active
}

/// Record first-seen timestamp for an alarm/info code
/// Returns the timestamp (existing or newly created)
pub fn record_first_seen(key: &str) -> SystemTime {
    let now = SystemTime::now();

    // Try to get existing timestamp
    if let Ok(cache) = ALARM_FIRST_SEEN.read()
        && let Some(&ts) = cache.get(key)
    {
        return ts;
    }

    // Insert new timestamp if not exists
    if let Ok(mut cache) = ALARM_FIRST_SEEN.write() {
        cache.entry(key.to_string()).or_insert(now);
    }

    // Return the timestamp (may have been inserted by another thread)
    if let Ok(cache) = ALARM_FIRST_SEEN.read()
        && let Some(&ts) = cache.get(key)
    {
        return ts;
    }

    now
}

/// Clean up first-seen entries for codes that are no longer active
pub fn cleanup_inactive_codes(active_codes: &std::collections::HashSet<String>) {
    if let Ok(mut cache) = ALARM_FIRST_SEEN.write() {
        cache.retain(|k, _| active_codes.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_latin1_to_utf8() {
        // Swedish characters in Latin-1
        let bytes = vec![0xE5, 0xE4, 0xF6]; // å, ä, ö
        let utf8 = latin1_to_utf8(&bytes);
        assert_eq!(utf8, "åäö");
    }

    #[test]
    fn test_parse_alarm_code_valid() {
        let input = "[E040] Low brine flow";
        let result = parse_alarm_code(input);
        assert!(result.is_ok());
        let (rest, code) = result.unwrap();
        assert_eq!(code, "E040");
        assert_eq!(rest, " Low brine flow");
    }

    #[test]
    fn test_parse_alarm_code_info() {
        let input = "[I019] Smart: Lågpris";
        let result = parse_alarm_code(input);
        assert!(result.is_ok());
        let (rest, code) = result.unwrap();
        assert_eq!(code, "I019");
        assert_eq!(rest, " Smart: Lågpris");
    }

    #[test]
    fn test_parse_alarm_code_invalid() {
        let input = "No code here";
        let result = parse_alarm_code(input);
        assert!(result.is_err());
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
    fn test_scan_bitmask_empty() {
        let registers = vec![0x0000, 0x0000];
        let active = scan_bitmask(&registers);
        assert!(active.is_empty());
    }

    #[test]
    fn test_scan_bitmask_single_bit() {
        let registers = vec![0x0001]; // Bit 0 set
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![0]);
    }

    #[test]
    fn test_scan_bitmask_multiple_bits() {
        let registers = vec![0x0005]; // Bits 0 and 2 set
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![0, 2]);
    }

    #[test]
    fn test_scan_bitmask_across_registers() {
        let registers = vec![0x0001, 0x0001]; // Bit 0 in first, bit 0 in second
        let active = scan_bitmask(&registers);
        assert_eq!(active, vec![0, 16]);
    }
}
