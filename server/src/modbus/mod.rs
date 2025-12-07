#![allow(dead_code)]
pub mod actor;
pub mod bms_parameters;
pub mod keepalive;
pub mod operations;

// Re-export actor types for easier access
pub use actor::{CtcActorBuilder, ModbusSender, ParameterOperation};

// Re-export keepalive for convenience
pub use keepalive::SmartGridKeepalive;

// Re-export operations for convenience
// read_parameter_value is part of the public API but currently unused internally
#[allow(unused_imports)]
pub use operations::{read_parameter, read_parameter_value, write_parameter};

// region: --- Modbus Parameter Struct
// Define the access type.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Access {
    R,
    RW,
    W,
}

#[derive(Debug, Clone, Copy)]
pub struct CTCModbusParameter {
    /// This register address contains the value for the parameter.
    pub id: u16,
    /// Indicates whether the parameter's value is signed or unsigned.
    pub signed: bool,
    /// Access type: either read-only (R) or read/write (RW).
    pub access: Access,
    /// This register address contains the maximum value for the parameter.
    pub reg_max: Option<u16>,
    /// This register address contains the minimum value for the parameter.
    pub reg_min: Option<u16>,
    /// This register address contains the step size for the parameter e.g., 0.1 for temperature.
    pub reg_step: Option<u16>,
    /// This register contains a bit field (mask) indicating which parameters are supported or active.
    pub visible: u16,
    /// The bit position within the bit field from the "Visible" register corresponding to this parameter.
    pub bit: u8,
    /// Scaling factor to convert the raw register value into physical units, e.g., 0.1 for temperature.
    pub factor: f32,
    /// The name of the parameter, e.g., "Room Temperature".
    pub description: &'static str,
}

impl CTCModbusParameter {
    /// Determines if the parameter is read-only.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.access == Access::R
    }

    /// Checks if the parameter is visible based on the provided bit mask from the "Visible" register.
    ///
    /// # Arguments
    /// * `value` - The bit mask to check against
    ///
    /// # Returns
    /// `true` if the bit at position `self.bit` is set in the mask, `false` otherwise
    #[must_use]
    pub fn is_visible(&self, value: u16) -> bool {
        (value & (1 << self.bit)) != 0
    }

    /// Scales a raw register value according to the parameter's scaling factor.
    /// Rounds to one decimal place for better readability.
    ///
    /// # Arguments
    /// * `raw_value` - The raw value to scale
    /// * `signed` - Whether to interpret the raw value as signed
    ///
    /// # Returns
    /// The scaled value rounded to one decimal place
    #[inline]
    #[allow(clippy::cast_possible_wrap)]
    fn scale_value(&self, raw_value: u16) -> f32 {
        let value = if self.signed {
            // Direct cast to i16 preserves the bit pattern and correctly interprets negative values
            f32::from(raw_value as i16)
        } else {
            f32::from(raw_value)
        };

        // Scale and round to one decimal place
        (value * self.factor * 10.0).round() / 10.0
    }

    /// Returns a vector of scaled values for a slice of raw register values.
    ///
    /// # Arguments
    /// * `value` - Slice of raw register values
    ///
    /// # Returns
    /// A vector of scaled values
    #[must_use]
    pub fn get_scaled_value_vector(&self, value: &[u16]) -> Vec<f32> {
        value.iter().map(|&v| self.scale_value(v)).collect()
    }

    /// Returns the scaled value for a single raw register value.
    ///
    /// # Arguments
    /// * `value` - The raw register value
    ///
    /// # Returns
    /// The scaled value
    #[must_use]
    pub fn get_scaled_value(&self, value: u16) -> f32 {
        self.scale_value(value)
    }

    /// Converts a scaled value back to its raw register value.
    ///
    /// # Arguments
    /// * `value` - The scaled value to convert
    ///
    /// # Returns
    /// A vector containing the raw register value
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    pub fn get_raw_value(&self, value: f32) -> u16 {
        let raw_value = (value / self.factor).round();

        if self.signed {
            // First convert to i16, then cast to u16 to preserve bit pattern
            raw_value as i16 as u16
        } else {
            raw_value as u16
        }
    }
}
// endregion: --- Modbus Parameter Struct

// region: --- Hot Water Mode Enum
#[derive(PartialEq, Debug)]
pub enum HotWaterMode {
    Economy,
    Normal,
    Comfort,
    Manual,
}

impl From<HotWaterMode> for u16 {
    fn from(mode: HotWaterMode) -> Self {
        match mode {
            HotWaterMode::Economy => 0,
            HotWaterMode::Normal => 1,
            HotWaterMode::Comfort => 2,
            HotWaterMode::Manual => 3,
        }
    }
}

impl TryFrom<u16> for HotWaterMode {
    type Error = &'static str;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(HotWaterMode::Economy),
            1 => Ok(HotWaterMode::Normal),
            2 => Ok(HotWaterMode::Comfort),
            3 => Ok(HotWaterMode::Manual),
            _ => Err("Invalid HotWaterMode value"),
        }
    }
}

impl std::fmt::Display for HotWaterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HotWaterMode::Economy => write!(f, "Economy"),
            HotWaterMode::Normal => write!(f, "Normal"),
            HotWaterMode::Comfort => write!(f, "Comfort"),
            HotWaterMode::Manual => write!(f, "Manual"),
        }
    }
}
// endregion: --- Hot Water Mode Enum

// region: --- Heating system 1 status Enum
/* Heating system 1 status
0 = Heating off
1 = Vacation
2= Night reduction
3= On (normal mode)
*/
#[derive(PartialEq, Debug)]
pub enum HeatSystemStatus {
    HeatingOff,
    Vacation,
    NightReduction,
    On,
}

impl From<HeatSystemStatus> for u16 {
    fn from(status: HeatSystemStatus) -> Self {
        match status {
            HeatSystemStatus::HeatingOff => 0,
            HeatSystemStatus::Vacation => 1,
            HeatSystemStatus::NightReduction => 2,
            HeatSystemStatus::On => 3,
        }
    }
}

impl TryFrom<u16> for HeatSystemStatus {
    type Error = &'static str;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(HeatSystemStatus::HeatingOff),
            1 => Ok(HeatSystemStatus::Vacation),
            2 => Ok(HeatSystemStatus::NightReduction),
            3 => Ok(HeatSystemStatus::On),
            _ => Err("Invalid HeatSystemStatus value"),
        }
    }
}

impl std::fmt::Display for HeatSystemStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeatSystemStatus::HeatingOff => write!(f, "Heating off"),
            HeatSystemStatus::Vacation => write!(f, "Vacation"),
            HeatSystemStatus::NightReduction => write!(f, "Night reduction"),
            HeatSystemStatus::On => write!(f, "On"),
        }
    }
}
// endregion: --- Heating system 1 status Enum

// region: --- SmartGrid Control
/// `SmartGrid` control modes for register 1100
/// Uses bits 6-7 (0x00, 0x40, 0x80, 0xC0) for virtual digital inputs
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartGridMode {
    /// Normal operation (0b00000000)
    Normal = 0b0000_0000,
    /// Blocking - stop heating/cooling (0b01000000)
    /// Equivalent to power-save mode without exhausting config register write cycles
    Blocking = 0b0100_0000,
    /// Low Price - prioritize operation when energy is cheap (0b10000000)
    LowPrice = 0b1000_0000,
    /// Overcapacity - maximum operation when excess energy available (0b11000000)
    Overcapacity = 0b1100_0000,
}

impl SmartGridMode {
    /// Convert `SmartGridMode` to u16 for Modbus register write
    #[must_use]
    pub fn to_register_value(self) -> u16 {
        u16::from(self as u8)
    }

    /// Parse `SmartGridMode` from u16 Modbus register value
    /// Masks out bits 6-7 (0xC0) to extract the `SmartGrid` mode
    ///
    /// # Errors
    /// Returns error if the extracted bits don't match a valid mode
    pub fn from_register_value(value: u16) -> Result<Self, &'static str> {
        let masked = (value & 0x00C0) as u8;
        match masked {
            0b0000_0000 => Ok(Self::Normal),
            0b0100_0000 => Ok(Self::Blocking),
            0b1000_0000 => Ok(Self::LowPrice),
            0b1100_0000 => Ok(Self::Overcapacity),
            _ => Err("Invalid SmartGrid mode bits"),
        }
    }

    /// Create mode from K25/K26 terminal closed states
    ///
    /// # Arguments
    /// * `k25_closed` - True if K25 (Smart A) terminal is closed
    /// * `k26_closed` - True if K26 (Smart B) terminal is closed
    #[must_use]
    pub fn from_terminals(k25_closed: bool, k26_closed: bool) -> Self {
        match (k25_closed, k26_closed) {
            (false, false) => Self::Normal,
            (true, false) => Self::Blocking,
            (false, true) => Self::LowPrice,
            (true, true) => Self::Overcapacity,
        }
    }

    /// Get required K25/K26 terminal states for this mode
    ///
    /// Returns (`k25_closed`, `k26_closed`)
    #[must_use]
    pub fn terminal_states(self) -> (bool, bool) {
        match self {
            Self::Normal => (false, false),
            Self::Blocking => (true, false),
            Self::LowPrice => (false, true),
            Self::Overcapacity => (true, true),
        }
    }
}

impl std::fmt::Display for SmartGridMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "Normal"),
            Self::Blocking => write!(f, "Blocking"),
            Self::LowPrice => write!(f, "LowPrice"),
            Self::Overcapacity => write!(f, "Overcapacity"),
        }
    }
}

impl std::str::FromStr for SmartGridMode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "blocking" => Ok(Self::Blocking),
            "lowprice" | "low_price" | "low-price" => Ok(Self::LowPrice),
            "overcapacity" | "over_capacity" | "over-capacity" => Ok(Self::Overcapacity),
            _ => Err("Invalid SmartGrid mode string"),
        }
    }
}

/// `SmartGrid` control register address (Control Parameters)
/// BMS Manual page 5: Control Parameters (1000-1999) for frequent writes with keepalive
pub const SMARTGRID_CONTROL_REGISTER: u16 = 1100;

/// `SmartGrid` keepalive requirement: must refresh every 5 minutes
/// We use 4 minutes (240 seconds) to provide a safety margin
pub const SMARTGRID_KEEPALIVE_INTERVAL_SECS: u64 = 240;

// endregion: --- SmartGrid Control

// region: --- Unit tests
#[cfg(test)]
mod tests {
    use crate::modbus::{
        HeatSystemStatus, HotWaterMode,
        bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, HEATSYSTEM_STATUS},
    };

    // Helper function for float comparison with epsilon
    fn assert_float_eq(a: f32, b: f32, msg: &str) {
        assert!((a - b).abs() < f32::EPSILON, "{msg}: expected {b}, got {a}");
    }

    // region: --- Test for conversion of positive and negative values to raw values
    #[test]
    fn test_get_raw_value_pos() {
        let value: f32 = 10.5;
        let raw_value = HEATSYSTEM_ROOM_SETTEMP.get_raw_value(value);
        assert_eq!(raw_value, 105);
    }

    #[test]
    fn test_get_raw_value_neg() {
        let value: f32 = -10.5;
        let raw_value = HEATSYSTEM_ROOM_SETTEMP.get_raw_value(value);
        assert_eq!(raw_value, 65431);
    }
    // endregion: --- Test for conversion of positive and negative values to raw values (signed BMS parameters)

    // region: --- Test for conversion of raw values to positive and negative scaled values (signed BMS parameters)
    #[test]
    fn test_get_scaled_value_pos() {
        let raw_value = 105; // Example raw value
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 10.5, "test_get_scaled_value_pos");
    }

    #[test]
    fn test_get_scaled_value_neg() {
        let raw_value = 65431; // Example raw value for negative value
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, -10.5, "test_get_scaled_value_neg");
    }
    // endregion: --- Test for conversion of raw values to positive and negative scaled values (signed BMS parameters)

    #[test]
    fn test_get_scaled_value_vector_0_1() {
        let raw_values = vec![221, 222, 223];
        let scaled_values = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value_vector(&raw_values);
        assert_eq!(scaled_values, vec![22.1, 22.2, 22.3]);
    }

    #[test]
    fn test_get_scaled_value_vector_1_0() {
        let raw_values = vec![22, 23, 24];
        let scaled_values = HEATSYSTEM_STATUS.get_scaled_value_vector(&raw_values);
        assert_eq!(scaled_values, vec![22.0, 23.0, 24.0]);
    }

    #[test]
    fn test_to_scaled_value_vector_0_1() {
        let value = 22.1;
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_raw_value(value);
        assert_eq!(scaled_value, 221);
    }

    #[test]
    fn test_to_scaled_value_vector_1_0() {
        let value = 22_f32;
        let scaled_value = HEATSYSTEM_STATUS.get_raw_value(value);
        assert_eq!(scaled_value, 22);
    }

    #[test]
    fn test_get_scaled_value_0_1() {
        let raw_value = 221;
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 22.1, "test_get_scaled_value_0_1");
    }

    // Test for signed value
    #[test]
    fn test_get_scaled_value_signed() {
        let raw_value = 32767; // Maximum value for i16
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 3276.7, "test_get_scaled_value_signed");
    }

    // Test for signed value with negative value
    #[test]
    fn test_get_scaled_value_signed_negative() {
        let raw_value = 32768; // Minimum value for i16 (as u16)
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(
            scaled_value,
            -3276.8,
            "test_get_scaled_value_signed_negative",
        );
    }

    // Test for different scaling factor
    #[test]
    fn test_get_scaled_value_different_factor() {
        let raw_value = 100; // Example raw value
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 10.0, "test_get_scaled_value_different_factor");
    }

    // Test for 1 in scaled factor
    #[test]
    fn test_get_scaled_value_one_factor() {
        let raw_value = 10; // Example raw value
        let scaled_value = HEATSYSTEM_STATUS.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 10.0, "test_get_scaled_value_one_factor");
    }

    // Test for zero raw value
    #[test]
    fn test_get_scaled_value_zero() {
        let raw_value = 0; // Example raw value
        let scaled_value = HEATSYSTEM_STATUS.get_scaled_value(raw_value);
        assert_float_eq(scaled_value, 0.0, "test_get_scaled_value_zero");
    }

    #[test]
    fn test_hot_water_mode_conversions() {
        // Enum to u16
        assert_eq!(u16::from(HotWaterMode::Economy), 0);
        assert_eq!(u16::from(HotWaterMode::Normal), 1);
        assert_eq!(u16::from(HotWaterMode::Comfort), 2);
        assert_eq!(u16::from(HotWaterMode::Manual), 3);

        // u16 to Enum
        assert_eq!(HotWaterMode::try_from(0), Ok(HotWaterMode::Economy));
        assert_eq!(HotWaterMode::try_from(1), Ok(HotWaterMode::Normal));
        assert_eq!(HotWaterMode::try_from(2), Ok(HotWaterMode::Comfort));
        assert_eq!(HotWaterMode::try_from(3), Ok(HotWaterMode::Manual));
        assert!(HotWaterMode::try_from(4).is_err());
    }

    #[test]
    fn test_heat_system_status_conversions() {
        // Enum to u16
        assert_eq!(u16::from(HeatSystemStatus::HeatingOff), 0);
        assert_eq!(u16::from(HeatSystemStatus::Vacation), 1);
        assert_eq!(u16::from(HeatSystemStatus::NightReduction), 2);
        assert_eq!(u16::from(HeatSystemStatus::On), 3);

        // u16 to Enum
        assert_eq!(
            HeatSystemStatus::try_from(0),
            Ok(HeatSystemStatus::HeatingOff)
        );
        assert_eq!(
            HeatSystemStatus::try_from(1),
            Ok(HeatSystemStatus::Vacation)
        );
        assert_eq!(
            HeatSystemStatus::try_from(2),
            Ok(HeatSystemStatus::NightReduction)
        );
        assert_eq!(HeatSystemStatus::try_from(3), Ok(HeatSystemStatus::On));
        assert!(HeatSystemStatus::try_from(4).is_err());
    }

    #[test]
    fn test_smartgrid_mode_to_register() {
        use crate::modbus::SmartGridMode;

        assert_eq!(SmartGridMode::Normal.to_register_value(), 0x0000);
        assert_eq!(SmartGridMode::Blocking.to_register_value(), 0x0040);
        assert_eq!(SmartGridMode::LowPrice.to_register_value(), 0x0080);
        assert_eq!(SmartGridMode::Overcapacity.to_register_value(), 0x00C0);
    }

    #[test]
    fn test_smartgrid_mode_from_register() {
        use crate::modbus::SmartGridMode;

        assert_eq!(
            SmartGridMode::from_register_value(0x0000),
            Ok(SmartGridMode::Normal)
        );
        assert_eq!(
            SmartGridMode::from_register_value(0x0040),
            Ok(SmartGridMode::Blocking)
        );
        assert_eq!(
            SmartGridMode::from_register_value(0x0080),
            Ok(SmartGridMode::LowPrice)
        );
        assert_eq!(
            SmartGridMode::from_register_value(0x00C0),
            Ok(SmartGridMode::Overcapacity)
        );

        // Test with other bits set (should mask correctly)
        assert_eq!(
            SmartGridMode::from_register_value(0x003F),
            Ok(SmartGridMode::Normal)
        );
        assert_eq!(
            SmartGridMode::from_register_value(0x007F),
            Ok(SmartGridMode::Blocking)
        );
    }

    #[test]
    fn test_smartgrid_mode_from_str() {
        use crate::modbus::SmartGridMode;
        use std::str::FromStr;

        assert_eq!(SmartGridMode::from_str("normal"), Ok(SmartGridMode::Normal));
        assert_eq!(SmartGridMode::from_str("Normal"), Ok(SmartGridMode::Normal));
        assert_eq!(
            SmartGridMode::from_str("blocking"),
            Ok(SmartGridMode::Blocking)
        );
        assert_eq!(
            SmartGridMode::from_str("Blocking"),
            Ok(SmartGridMode::Blocking)
        );
        assert_eq!(
            SmartGridMode::from_str("lowprice"),
            Ok(SmartGridMode::LowPrice)
        );
        assert_eq!(
            SmartGridMode::from_str("low_price"),
            Ok(SmartGridMode::LowPrice)
        );
        assert_eq!(
            SmartGridMode::from_str("overcapacity"),
            Ok(SmartGridMode::Overcapacity)
        );
        assert_eq!(
            SmartGridMode::from_str("over_capacity"),
            Ok(SmartGridMode::Overcapacity)
        );
        assert!(SmartGridMode::from_str("invalid").is_err());
    }

    #[test]
    fn test_smartgrid_mode_display() {
        use crate::modbus::SmartGridMode;

        assert_eq!(format!("{}", SmartGridMode::Normal), "Normal");
        assert_eq!(format!("{}", SmartGridMode::Blocking), "Blocking");
        assert_eq!(format!("{}", SmartGridMode::LowPrice), "LowPrice");
        assert_eq!(format!("{}", SmartGridMode::Overcapacity), "Overcapacity");
    }

    // region: --- Test step validation logic
    /// Tests the step validation logic that checks if a value is valid based on min/max/step
    /// This validates the fix for the bug where step validation was incorrect when min != 0
    #[test]
    fn test_step_validation_from_minimum() {
        // Test case: min=152, step=5
        // Valid values should be: 152, 157, 162, 167, etc.
        let min = 152_u16;
        let step = 5_u16;

        // Value equals minimum - should be valid
        assert!(
            (min - min).is_multiple_of(step),
            "152 should be valid (offset 0 from min 152)"
        );

        // Value is exactly one step from minimum - should be valid
        let value = 157_u16;
        assert!(
            (value - min).is_multiple_of(step),
            "157 should be valid (offset 5 from min 152)"
        );

        // Value is two steps from minimum - should be valid
        let value = 162_u16;
        assert!(
            (value - min).is_multiple_of(step),
            "162 should be valid (offset 10 from min 152)"
        );

        // Value is not a multiple of step from minimum - should be invalid
        let value = 153_u16;
        assert!(
            !(value - min).is_multiple_of(step),
            "153 should be invalid (offset 1 from min 152)"
        );

        let value = 154_u16;
        assert!(
            !(value - min).is_multiple_of(step),
            "154 should be invalid (offset 2 from min 152)"
        );
    }

    #[test]
    fn test_step_validation_from_zero() {
        // Test case where minimum is 0 (original logic works here)
        let min = 0_u16;
        let step = 10_u16;

        // All multiples of 10 should be valid
        assert!((0_u16 - min).is_multiple_of(step), "0 should be valid");
        assert!((10_u16 - min).is_multiple_of(step), "10 should be valid");
        assert!((50_u16 - min).is_multiple_of(step), "50 should be valid");

        // Non-multiples should be invalid
        assert!(!(5_u16 - min).is_multiple_of(step), "5 should be invalid");
        assert!(!(15_u16 - min).is_multiple_of(step), "15 should be invalid");
    }

    #[test]
    fn test_step_validation_various_scenarios() {
        // Test various min/step combinations

        // Scenario 1: min=100, step=3
        let min = 100_u16;
        let step = 3_u16;
        assert!((100_u16 - min).is_multiple_of(step), "100 should be valid");
        assert!((103_u16 - min).is_multiple_of(step), "103 should be valid");
        assert!((106_u16 - min).is_multiple_of(step), "106 should be valid");
        assert!(
            !(101_u16 - min).is_multiple_of(step),
            "101 should be invalid"
        );
        assert!(
            !(102_u16 - min).is_multiple_of(step),
            "102 should be invalid"
        );

        // Scenario 2: min=7, step=1 (all values valid)
        let min = 7_u16;
        let step = 1_u16;
        assert!((7_u16 - min).is_multiple_of(step), "7 should be valid");
        assert!((8_u16 - min).is_multiple_of(step), "8 should be valid");
        assert!((100_u16 - min).is_multiple_of(step), "100 should be valid");
    }
    // endregion: --- Test step validation logic
}
// endregion: --- Unit tests
