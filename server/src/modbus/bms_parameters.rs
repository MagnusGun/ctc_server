// Define the modbus parameters for the CTC Heating System 1.
// see 'Service document-BMS Register-17003548.pdf'
#![allow(dead_code)]
use crate::modbus::{Access, CTCModbusParameter};
use std::collections::HashMap;
use std::sync::OnceLock;

/// This module provides constants and utilities for CTC Modbus parameters.
///
/// # Macro Usage
/// The `ctc_parameter!` macro is used to create parameter definitions with minimal boilerplate:
///
/// ## Examples
///
/// For read-write parameters:
/// ```rust
/// // Format: ctc_parameter!(NAME, ID, DESCRIPTION, FACTOR, ACCESS, REG_BASE, VISIBLE, BIT)
/// ctc_parameter!(HEATSYSTEM_ROOM_SETTEMP, 61509, "Heating system 1: Set room temperature", 0.1, Access::RW, 60027, 62500, 9);
/// ```
///
/// For read-only parameters:
/// ```rust
/// // Format: ctc_parameter!(NAME, ID, DESCRIPTION, FACTOR, VISIBLE, BIT)
/// ctc_parameter!(HEATSYSTEM_FLOW_TEMP, 62011, "Heating system 1: Primary flow temperature", 0.1, 62531, 15);
/// ```
// Macro for creating CTC parameters with minimal boilerplate
macro_rules! ctc_parameter {
    // Simplified version for read-write parameters with sequential registers
    ($name:ident, $id:expr, $desc:expr, $factor:expr, $access:expr,
     $reg_base:expr, $visible:expr, $bit:expr) => {
        pub const $name: CTCModbusParameter = CTCModbusParameter {
            id: $id,
            signed: true,
            access: $access,
            reg_max: Some($reg_base),
            reg_min: Some($reg_base + 1),
            reg_step: Some($reg_base + 2),
            visible: $visible,
            bit: $bit,
            factor: $factor,
            description: $desc,
        };
    };

    // For read-only parameters (simpler signature, no reg_* values needed)
    ($name:ident, $id:expr, $desc:expr, $factor:expr, $visible:expr, $bit:expr) => {
        pub const $name: CTCModbusParameter = CTCModbusParameter {
            id: $id,
            signed: true,
            access: Access::R,
            reg_max: None,
            reg_min: None,
            reg_step: None,
            visible: $visible,
            bit: $bit,
            factor: $factor,
            description: $desc,
        };
    };
}

// Variant for read-only parameters whose raw register value is unsigned
// (e.g. status/code registers where bit 15 must not be sign-extended).
macro_rules! ctc_parameter_unsigned {
    ($name:ident, $id:expr, $desc:expr, $factor:expr, $visible:expr, $bit:expr) => {
        pub const $name: CTCModbusParameter = CTCModbusParameter {
            id: $id,
            signed: false,
            access: Access::R,
            reg_max: None,
            reg_min: None,
            reg_step: None,
            visible: $visible,
            bit: $bit,
            factor: $factor,
            description: $desc,
        };
    };
}
// region: --- CTC Heating System 1 Modbus Parameters

ctc_parameter!(
    HEATSYSTEM_ROOM_SETTEMP,
    61509,
    "Heating system 1: Set room temperature",
    0.1,
    Access::RW,
    60027,
    62500,
    9
);
ctc_parameter!(
    HEATSYSTEM_INCLINATION,
    61513,
    "Heating system 1: Change inclination",
    0.1,
    Access::RW,
    60039,
    62500,
    13
);
ctc_parameter!(
    HEATSYSTEM_ADJUSTMENT,
    61517,
    "Heating system 1: Change adjustment",
    0.1,
    Access::RW,
    60051,
    62501,
    1
);
ctc_parameter!(
    HEATSYSTEM_FLOW_MAX_TEMP,
    61534,
    "Heating system 1: Max Primary flow °C",
    0.1,
    Access::RW,
    60102,
    62502,
    2
);
ctc_parameter!(
    HEATSYSTEM_FLOW_MIN_TEMP,
    61538,
    "Heating system 1: Min primary flow °C",
    0.1,
    Access::RW,
    60114,
    62502,
    6
);
ctc_parameter!(
    HEATSYSTEM_HEATING_MODE,
    61542,
    "Heating system 1: Heating mode",
    1.0,
    Access::RW,
    60126,
    62502,
    10
);
ctc_parameter!(
    HEATSYSTEM_HEAT_OFF_TEMP,
    61546,
    "Heating system 1: Heating off, out °C",
    0.1,
    Access::RW,
    60138,
    62502,
    14
);
ctc_parameter!(
    HEATSYSTEM_HEAT_OFF_TIME,
    61550,
    "Heating system 1: Heating off time",
    1.0,
    Access::RW,
    60150,
    62503,
    2
);
ctc_parameter!(
    HEATSYSTEM_ROOM_TEMP_NIGHT_REDUCTION,
    61554,
    "Heating system 1: Room temp night reduction",
    0.1,
    Access::RW,
    60162,
    62503,
    6
);
ctc_parameter!(
    HEATSYSTEM_OUTDOOR_NIGHT_REDUCTION,
    61562,
    "Heating system 1: Outdoor temp night reduction",
    0.1,
    Access::RW,
    60186,
    62503,
    14
);
ctc_parameter!(
    HEATSYSTEM_ALARM_LOW_ROOM_TEMP,
    61566,
    "Heating system 1: Alarm low room temperature",
    0.1,
    Access::RW,
    60198,
    62504,
    2
);
ctc_parameter!(
    HEATSYSTEM_FLOW_SETPOINT,
    62007,
    "Heating system 1: Temperature setpoint primary flow",
    0.1,
    62531,
    11
);
ctc_parameter!(
    HEATSYSTEM_FLOW_TEMP,
    62011,
    "Heating system 1: Primary flow temperature",
    0.1,
    62531,
    15
);
ctc_parameter_unsigned!(
    HEATSYSTEM_STATUS,
    62246,
    "Heating system 1 status",
    1.0,
    62546,
    10
);

// endregion: --- CTC Heating System 1 Modbus Parameters

// region: --- CTC Common Modbus Parameters

ctc_parameter!(CTC_RETURN_TEMP, 62015, "Return temp", 0.1, 62532, 3);
ctc_parameter!(
    CTC_HOT_WATER_MODE,
    61500,
    "Hot water mode",
    1.0,
    Access::RW,
    60000,
    62500,
    0
);
ctc_parameter!(CTC_ROOM_TEMP, 62203, "Current room temp 1", 0.1, 62543, 15);
ctc_parameter!(
    CTC_OUTDOOR_TEMP,
    62000,
    "Outdoor temperature",
    0.1,
    62531,
    4
);
ctc_parameter!(
    CTC_VACCATION_DAYS,
    61508,
    "Number of vacation days timer",
    1.0,
    Access::RW,
    60024,
    62500,
    8
);
ctc_parameter!(
    CTC_STOP_TEMP_DHW,
    62001,
    "Stop temperature DHW",
    0.1,
    62531,
    5
);
ctc_parameter!(
    CTC_DELAY_MIXING_VALVE,
    62004,
    "Delay mixing valve",
    1.0,
    62531,
    8
);
ctc_parameter_unsigned!(CTC_SYSTEM_STATUS, 62005, "System status", 1.0, 62531, 9);
// Radiator water temperature (also measures lower tank temperature)
ctc_parameter!(CTC_RADIATOR_WATER, 62006, "Radiator water", 0.1, 62531, 10);
ctc_parameter!(CTC_PRODUCT_TYPE, 62253, "Product type", 1.0, 62547, 1);

// endregion: --- CTC Common Modbus Parameters

// region: --- CTC Hot Water Modbus Parameters

ctc_parameter!(
    CTC_HOT_WATER_STOP_TEMP,
    61501,
    "Manual stop temperature hot water",
    0.1,
    Access::RW,
    60003,
    62500,
    1
);
ctc_parameter!(
    CTC_EXTRA_HOT_WATER_TIMER,
    61503,
    "Extra hot water timer",
    0.5,
    Access::RW,
    60009,
    62500,
    3
);
ctc_parameter!(
    CTC_MAX_TIME_HEATING_HP,
    61504,
    "Maximum time heating heat pump",
    1.0,
    Access::RW,
    60012,
    62500,
    4
);
ctc_parameter!(
    CTC_MAX_TIME_HOT_WATER,
    61505,
    "Maximum time hot water",
    1.0,
    Access::RW,
    60015,
    62500,
    5
);
ctc_parameter!(
    CTC_SETPOINT_LOWER_TANK,
    62274,
    "Setpoint lower tank",
    0.1,
    62548,
    6
);
ctc_parameter!(
    CTC_ACTUAL_TEMP_DHW,
    62276,
    "Actual temperature DHW",
    0.1,
    62548,
    8
);

// endregion: --- CTC Hot Water Modbus Parameters

// region: --- CTC Immersion Heater Modbus Parameters

ctc_parameter!(
    CTC_MAX_IMMERSION_HEATER_DHW,
    61591,
    "Max immersion heater DHW kW",
    0.1,
    Access::RW,
    60273,
    62505,
    11
);

// endregion: --- CTC Immersion Heater Modbus Parameters

// region: --- CTC Mixing Valve Modbus Parameters

ctc_parameter!(
    CTC_DELAY_MIXING_VALVE_SETTING,
    61629,
    "Delay mixing valve setting",
    1.0,
    Access::RW,
    60387,
    62508,
    1
);

// endregion: --- CTC Mixing Valve Modbus Parameters

// region: --- CTC HeatPump 1 Modbus Parameters

ctc_parameter!(
    HEATPUMP_BLOCKED,
    61521,
    "Heat pump 1 (A1): Blocked",
    1.0,
    Access::RW,
    60063,
    62501,
    5
);
ctc_parameter_unsigned!(
    HEATPUMP_STATUS,
    62017,
    "Heat pump 1 (A1): Status",
    1.0,
    62532,
    5
);
ctc_parameter!(
    HEATPUMP_INLET_TEMP,
    62027,
    "Heat pump 1 (A1) HP in",
    0.1,
    62532,
    15
);
ctc_parameter!(
    HEATPUMP_OUTLET_TEMP,
    62037,
    "Heat pump 1 (A1) HP out",
    0.1,
    62533,
    9
);
ctc_parameter!(
    HEATPUMP_DISCHARGE_TEMP,
    62047,
    "Heat pump 1 (A1): Discharge temperature",
    0.1,
    62534,
    3
);
ctc_parameter!(
    HEATPUMP_SUCTION_TEMP,
    62057,
    "Heat pump 1 (A1): Suction gas temperature",
    0.1,
    62534,
    13
);
ctc_parameter!(
    HEATPUMP_HIGH_PRESSURE,
    62067,
    "Heat pump 1 (A1): High pressure",
    0.1,
    62535,
    7
);
ctc_parameter!(
    HEATPUMP_LOW_PRESSURE,
    62077,
    "Heat pump 1 (A1): Low Pressure",
    0.1,
    62536,
    1
);
ctc_parameter!(
    HEATPUMP_BRINE_INLET_TEMP,
    62087,
    "Heat pump 1 (A1): Brine in",
    0.1,
    62536,
    11
);
ctc_parameter!(
    HEATPUMP_BRINE_OUTLET_TEMP,
    62097,
    "Heat pump 1 (A1): Brine out",
    0.1,
    62537,
    5
);
ctc_parameter!(
    HEATPUMP_CHARGE_PUMP,
    62107,
    "Heat pump 1 (A1): Charge pump",
    0.1,
    62537,
    15
);
ctc_parameter!(
    HEATPUMP_BRINE_PUMP,
    62117,
    "Heat pump 1 (A1): Brine pump",
    0.1,
    62538,
    9
);
ctc_parameter!(
    HEATPUMP_SOFTWARE_VERSION,
    62157,
    "Heat pump 1 (A1): Software version",
    1.0,
    62541,
    1
);
ctc_parameter!(HEATPUMP_TYPE, 62254, "Heat pump 1 (A1) Type", 1.0, 62547, 2);
ctc_parameter!(
    HEATPUMP_COMPRESSOR_MODEL,
    62264,
    "Heat pump 1 (A1) compressor model",
    1.0,
    62547,
    12
);

// endregion: --- CTC HeatPump 1 Modbus Parameters

// region: --- CTC Power & Current Modbus Parameters

ctc_parameter!(
    CTC_POWER_IMMERSION_HEATER,
    62168,
    "Power kW immersion heater",
    0.1,
    62541,
    12
);
ctc_parameter!(CTC_CURRENT_L1, 62171, "Current L1", 0.1, 62541, 15);
ctc_parameter!(CTC_CURRENT_L2, 62172, "Current L2", 0.1, 62542, 0);
ctc_parameter!(CTC_CURRENT_L3, 62173, "Current L3", 0.1, 62542, 1);

// endregion: --- CTC Power & Current Modbus Parameters

// region: --- CTC Statistics Modbus Parameters

ctc_parameter!(
    CTC_STAT_TOTAL_OPERATION_LSB,
    62186,
    "Total operation hours LSB",
    1.0,
    62542,
    14
);
ctc_parameter!(
    CTC_STAT_IMMERSION_HEATER_KWH,
    62191,
    "Immersion heater kWh",
    1.0,
    62543,
    3
);
ctc_parameter!(CTC_FUNCTION_TEST, 62192, "Function test", 1.0, 62543, 4);
ctc_parameter!(
    CTC_COMPRESSOR_OPERATION_TIME_LSB,
    62214,
    "Compressor 1 operating time LSB",
    1.0,
    62544,
    10
);
ctc_parameter!(
    CTC_COMPRESSOR_LAST_24H,
    62234,
    "Compressor 1 last 24h",
    1.0,
    62545,
    14
);

// endregion: --- CTC Statistics Modbus Parameters

// region: --- CTC System Info Modbus Parameters

ctc_parameter!(
    CTC_SOFTWARE_VERSION_MONTH_DAY,
    62244,
    "Software version display month day",
    1.0,
    62546,
    8
);
ctc_parameter!(
    CTC_SOFTWARE_VERSION_YEAR,
    62245,
    "Software version display year",
    1.0,
    62546,
    9
);

// endregion: --- CTC System Info Modbus Parameters

/// Alarm/info count register (always visible)
/// Lower byte = alarm count, Upper byte = info count
pub const CTC_ALARM_INFO_COUNT: CTCModbusParameter = CTCModbusParameter {
    id: 65001,
    signed: false,
    access: Access::R,
    reg_max: None,
    reg_min: None,
    reg_step: None,
    visible: 0, // Always visible (no visibility register)
    bit: 0,
    factor: 1.0,
    description: "Active alarm and info count",
};

/// Lowest valid alarm reference value (inclusive) for the text buffer transfer register.
pub const ALARM_REF_MIN: u16 = 0;
/// Highest valid alarm reference value (inclusive) for the text buffer transfer register.
pub const ALARM_REF_MAX: u16 = 9999;
/// Offset added to an info index to form its reference value (info N = `INFO_REF_OFFSET` + N).
pub const INFO_REF_OFFSET: u16 = 10000;
/// Highest valid info reference value (inclusive) for the text buffer transfer register.
pub const INFO_REF_MAX: u16 = 19999;

/// Transfer alarm/info reference into text buffer (write-only)
/// Values `ALARM_REF_MIN`-`ALARM_REF_MAX`: Alarm number (0 = Alarm number 0)
/// Values `INFO_REF_OFFSET`-`INFO_REF_MAX`: Info number (`INFO_REF_OFFSET` = Info number 0)
pub const CTC_ALARM_INFO_BUFFER: CTCModbusParameter = CTCModbusParameter {
    id: 65100,
    signed: false,
    access: Access::W,
    reg_max: None,
    reg_min: None,
    reg_step: None,
    visible: 0, // Always visible (no visibility register)
    bit: 0,
    factor: 1.0,
    description: "Transfer alarm/info reference to text buffer",
};

/// Returns all CTC Modbus parameters as a slice
fn all_ctc_parameters() -> &'static [&'static CTCModbusParameter] {
    static PARAMETERS: OnceLock<Vec<&'static CTCModbusParameter>> = OnceLock::new();
    PARAMETERS
        .get_or_init(|| {
            vec![
                // Heating System parameters
                &HEATSYSTEM_ROOM_SETTEMP,
                &HEATSYSTEM_INCLINATION,
                &HEATSYSTEM_ADJUSTMENT,
                &HEATSYSTEM_FLOW_MAX_TEMP,
                &HEATSYSTEM_FLOW_MIN_TEMP,
                &HEATSYSTEM_HEATING_MODE,
                &HEATSYSTEM_HEAT_OFF_TEMP,
                &HEATSYSTEM_HEAT_OFF_TIME,
                &HEATSYSTEM_ROOM_TEMP_NIGHT_REDUCTION,
                &HEATSYSTEM_OUTDOOR_NIGHT_REDUCTION,
                &HEATSYSTEM_ALARM_LOW_ROOM_TEMP,
                &HEATSYSTEM_FLOW_SETPOINT,
                &HEATSYSTEM_FLOW_TEMP,
                &HEATSYSTEM_STATUS,
                // Common parameters
                &CTC_RETURN_TEMP,
                &CTC_HOT_WATER_MODE,
                &CTC_ROOM_TEMP,
                &CTC_OUTDOOR_TEMP,
                &CTC_VACCATION_DAYS,
                &CTC_STOP_TEMP_DHW,
                &CTC_DELAY_MIXING_VALVE,
                &CTC_SYSTEM_STATUS,
                &CTC_RADIATOR_WATER,
                &CTC_PRODUCT_TYPE,
                // Hot water parameters
                &CTC_HOT_WATER_STOP_TEMP,
                &CTC_EXTRA_HOT_WATER_TIMER,
                &CTC_MAX_TIME_HEATING_HP,
                &CTC_MAX_TIME_HOT_WATER,
                &CTC_SETPOINT_LOWER_TANK,
                &CTC_ACTUAL_TEMP_DHW,
                // Immersion heater parameters
                &CTC_MAX_IMMERSION_HEATER_DHW,
                // Mixing valve parameters
                &CTC_DELAY_MIXING_VALVE_SETTING,
                // Heat Pump parameters
                &HEATPUMP_BLOCKED,
                &HEATPUMP_STATUS,
                &HEATPUMP_INLET_TEMP,
                &HEATPUMP_OUTLET_TEMP,
                &HEATPUMP_DISCHARGE_TEMP,
                &HEATPUMP_SUCTION_TEMP,
                &HEATPUMP_HIGH_PRESSURE,
                &HEATPUMP_LOW_PRESSURE,
                &HEATPUMP_BRINE_INLET_TEMP,
                &HEATPUMP_BRINE_OUTLET_TEMP,
                &HEATPUMP_CHARGE_PUMP,
                &HEATPUMP_BRINE_PUMP,
                &HEATPUMP_SOFTWARE_VERSION,
                &HEATPUMP_TYPE,
                &HEATPUMP_COMPRESSOR_MODEL,
                // Power & current parameters
                &CTC_POWER_IMMERSION_HEATER,
                &CTC_CURRENT_L1,
                &CTC_CURRENT_L2,
                &CTC_CURRENT_L3,
                // Statistics parameters
                &CTC_STAT_TOTAL_OPERATION_LSB,
                &CTC_STAT_IMMERSION_HEATER_KWH,
                &CTC_FUNCTION_TEST,
                &CTC_COMPRESSOR_OPERATION_TIME_LSB,
                &CTC_COMPRESSOR_LAST_24H,
                // System info parameters
                &CTC_SOFTWARE_VERSION_MONTH_DAY,
                &CTC_SOFTWARE_VERSION_YEAR,
                // Alarm/info parameters
                &CTC_ALARM_INFO_COUNT,
                &CTC_ALARM_INFO_BUFFER,
            ]
        })
        .as_slice()
}

/// Returns a `HashMap` of all CTC Modbus parameters by their ID
fn ctc_parameters_by_id() -> &'static HashMap<u16, &'static CTCModbusParameter> {
    static PARAMETERS_MAP: OnceLock<HashMap<u16, &'static CTCModbusParameter>> = OnceLock::new();
    PARAMETERS_MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for &param in all_ctc_parameters() {
            map.insert(param.id, param);
        }
        map
    })
}

#[must_use]
pub fn get_ctc_parameter_by_id(id: u16) -> Option<&'static CTCModbusParameter> {
    ctc_parameters_by_id().get(&id).copied()
}

#[must_use]
pub fn get_custom_ctc_parameter_by_addr(addr: u16, factor: Option<f32>) -> CTCModbusParameter {
    CTCModbusParameter {
        id: addr,
        signed: true,
        access: Access::R,
        reg_max: None,
        reg_min: None,
        reg_step: None,
        // Custom reads bypass the visibility scan: visible == 0 means always-visible.
        visible: 0,
        bit: 0,
        factor: factor.unwrap_or(1.0),
        description: "Custom CTC Parameter",
    }
}
