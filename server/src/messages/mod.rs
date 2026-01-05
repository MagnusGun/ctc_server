//! Alarm and info message handling
//!
//! This module contains translations, types, and utilities for processing
//! alarm codes (E-codes) and info codes (I-codes) from the CTC heating system.

pub mod translations;
pub mod types;

// Re-export commonly used items
pub use translations::{ALARM_DESCRIPTIONS, ALARM_DESCRIPTIONS_SV, ALARM_TRANSLATIONS};
pub use types::{
    ALARM_TEXT_CACHE, AlarmMessage, CachedAlarmText, cleanup_inactive_codes, decode_text_buffer,
    parse_alarm_text, record_first_seen, scan_bitmask,
};
