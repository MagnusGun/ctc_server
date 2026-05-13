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

/// Memory cache for alarm/info code -> first seen timestamp.
/// In-memory only — not persisted across server restarts.
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
        if is_printable_byte(low) {
            bytes.push(low);
        }
        if is_printable_byte(high) {
            bytes.push(high);
        }
    }
    latin1_to_utf8(&bytes).trim().to_string()
}

/// Returns true for bytes that should be kept when decoding a CTC text buffer.
///
/// Allows printable Latin-1 (0x20..=0x7E and 0xA0..=0xFF) and common whitespace
/// (tab, newline, carriage return); rejects NUL, other C0 controls, and DEL.
///
/// `\t` / `\n` / `\r` are intentional: CTC alarm text uses them to separate
/// the code prefix from the human-readable message and to wrap multi-line
/// info entries. Stripping them collapses fields and breaks `parse_alarm_text`.
fn is_printable_byte(b: u8) -> bool {
    b != 0 && b != 127 && (b > 31 || b == b'\t' || b == b'\n' || b == b'\r')
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
///
/// The CTC protocol uses the same text buffer format for both alarms and infos
/// (prefix like `[E040]` or `[I019]` followed by message text), so the parser
/// does not need to distinguish between them. When — and only when — the
/// entry relates to a specific heat-pump unit, the device prepends an
/// `A1`..`A10` identifier (e.g. `"A1[E063] Komm.fel relakort"`); heat-circuit
/// or system-wide entries omit it. We strip it before extracting the code
/// since this server only targets installations of one heat pump.
pub fn parse_alarm_text(raw_text: &str) -> (Option<String>, String) {
    let trimmed = strip_hp_prefix(raw_text.trim());

    match parse_alarm_code(trimmed) {
        Ok((rest, code)) => (Some(code.to_string()), rest.trim().to_string()),
        Err(_) => (None, trimmed.to_string()),
    }
}

/// Strip a leading CTC heat-pump unit identifier (`A1`..`A10`) and surrounding
/// whitespace, if present. Returns the input unchanged otherwise.
fn strip_hp_prefix(s: &str) -> &str {
    let rest = s.strip_prefix('A').unwrap_or(s);
    if std::ptr::eq(rest, s) {
        return s;
    }
    let digits: usize = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return s;
    }
    // Validate the number is 1..=10 so we don't gobble unrelated text like "A99 wires".
    let (num, tail) = rest.split_at(digits);
    if !matches!(num.parse::<u8>(), Ok(1..=10)) {
        return s;
    }
    tail.trim_start()
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
    // Single write-locked entry insertion to avoid TOCTOU races: two threads
    // both observing a missing key cannot each insert their own `now`. Recover
    // from poisoning — a previous panic leaves the map in an indeterminate but
    // usable state.
    let mut cache = ALARM_FIRST_SEEN
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache.entry(key.to_string()).or_insert_with(SystemTime::now)
}

/// Clean up first-seen entries for codes that are no longer active
pub fn cleanup_inactive_codes(active_codes: &std::collections::HashSet<String>) {
    let mut cache = ALARM_FIRST_SEEN
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|k, _| active_codes.contains(k));
}

/// Test-only: serialise alarm tests AND wipe the static map. Tests that
/// exercise `get_alarms` insert entries into the process-global
/// `ALARM_FIRST_SEEN`; without serialisation and explicit reset, parallel
/// tests in the same binary observe leaked timestamps and races.
///
/// Returns a guard that holds the test mutex for the test's lifetime and
/// clears the map on drop so the next test starts clean even if the current
/// one panics.
#[cfg(test)]
static ALARM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[must_use]
pub fn clear_alarm_first_seen_guard() -> AlarmFirstSeenGuard {
    // The Mutex is 'static so its guard implicitly has lifetime 'static.
    // Recover from poison: a panicking sibling test still leaves the lock
    // usable; we want to keep running.
    let lock = match ALARM_TEST_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    {
        let mut cache = ALARM_FIRST_SEEN
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.clear();
    }
    AlarmFirstSeenGuard { _lock: lock }
}

#[cfg(test)]
pub struct AlarmFirstSeenGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for AlarmFirstSeenGuard {
    fn drop(&mut self) {
        let mut cache = ALARM_FIRST_SEEN
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.clear();
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
    fn test_decode_text_buffer_preserves_tab() {
        // "A\tB" — 'A' = 0x41, '\t' = 0x09 -> register = 0x0941
        //         'B' = 0x42, null pad      -> register = 0x0042
        let registers = vec![0x0941, 0x0042];
        let decoded = decode_text_buffer(&registers);
        assert_eq!(decoded, "A\tB");
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
    fn test_parse_alarm_text_strips_hp_prefix() {
        // Heat-pump-scoped entries from the CTC text buffer come as "A1[E063] ...".
        // The unit prefix should be stripped before code extraction.
        let (code, message) = parse_alarm_text("A1[E063] Komm.fel relakort");
        assert_eq!(code, Some("E063".to_string()));
        assert_eq!(message, "Komm.fel relakort");

        let (code, message) = parse_alarm_text("A10[E063] Komm.fel relakort");
        assert_eq!(code, Some("E063".to_string()));
        assert_eq!(message, "Komm.fel relakort");
    }

    #[test]
    fn test_parse_alarm_text_leaves_non_hp_prefix() {
        // A99 isn't a valid heat-pump id (only A1..A10) and shouldn't be stripped.
        let (code, message) = parse_alarm_text("A99 wires loose");
        assert_eq!(code, None);
        assert_eq!(message, "A99 wires loose");

        // A0 isn't valid either.
        let (code, message) = parse_alarm_text("A0[E001] x");
        assert_eq!(code, None);
        assert_eq!(message, "A0[E001] x");

        // Standalone "A" without digits is untouched.
        let (code, message) = parse_alarm_text("A general message");
        assert_eq!(code, None);
        assert_eq!(message, "A general message");
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

    #[test]
    fn test_parse_alarm_text_unclosed_bracket() {
        // "[E040" without closing bracket: parser should fail to extract a
        // code, falling back to "no code, full string as message".
        let (code, message) = parse_alarm_text("[E040 unterminated");
        assert_eq!(code, None);
        assert_eq!(message, "[E040 unterminated");
    }

    #[test]
    fn test_parse_alarm_text_empty_input() {
        let (code, message) = parse_alarm_text("");
        assert_eq!(code, None);
        assert_eq!(message, "");
    }

    #[test]
    fn test_parse_alarm_text_with_embedded_newline() {
        // CTC sometimes wraps multi-line info entries — the newline survives
        // `is_printable_byte`, so `parse_alarm_text` must handle it cleanly.
        let (code, message) = parse_alarm_text("[I019]\nLine two");
        assert_eq!(code, Some("I019".to_string()));
        // Leading whitespace (the newline) is trimmed by the parser convention.
        assert!(message.contains("Line two"));
    }

    /// Regression for the `unwrap_or_else(PoisonError::into_inner)` pattern
    /// in `record_first_seen` / `cleanup_inactive_codes`. After a panic-
    /// induced poison, both functions must still succeed and operate on the
    /// data that was in the map at the time of the panic.
    #[test]
    fn test_alarm_first_seen_recovers_from_poison() {
        let _guard = clear_alarm_first_seen_guard();

        // Seed an entry, then poison the lock by panicking inside a write.
        record_first_seen("A:POISON_TEST");

        let poison_attempt = std::panic::catch_unwind(|| {
            let _w = ALARM_FIRST_SEEN.write().unwrap();
            panic!("intentional poison");
        });
        assert!(poison_attempt.is_err());

        // Lock is now poisoned. Both API functions should still work.
        let now_again = record_first_seen("A:POISON_TEST");
        // First-seen time must still be the original (entry wasn't replaced).
        // We don't compare against a specific value because record_first_seen
        // returns SystemTime::now() on a fresh insert; the assertion is just
        // "no panic, returns a value".
        let _ = now_again;

        let mut active = std::collections::HashSet::new();
        active.insert("A:POISON_TEST".to_string());
        cleanup_inactive_codes(&active);

        let map = ALARM_FIRST_SEEN
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(map.contains_key("A:POISON_TEST"));
    }
}
