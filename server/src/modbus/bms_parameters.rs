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
    HEATSYSTEM_FLOW_NIGHT_REDUCTION,
    61558,
    "Heating system 1: Primary flow Night reduction",
    0.1,
    Access::RW,
    60174,
    62503,
    10
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
    HEATSYSTEM_HOLIDAY_REDUCTION,
    61602,
    "Heating system 1: Holiday reduction",
    0.1,
    Access::RW,
    60306,
    62506,
    6
);
ctc_parameter!(
    HEATSYSTEM_FLOW_HOLIDAY_REDUCTION,
    61606,
    "Heating system 1: Primary flow Holiday reduction",
    0.1,
    Access::RW,
    60318,
    62506,
    10
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
ctc_parameter!(
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

// endregion: --- CTC Common Modbus Parameters

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
ctc_parameter!(
    HEATPUMP_MAX_RMP,
    61572,
    "Heat pump 1 (A1): Max RPS",
    0.1,
    Access::RW,
    60216,
    62504,
    8
);
ctc_parameter!(
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
    HEATPUMP_FAN_SPEED,
    62127,
    "Heat pump 1 (A1): Fan",
    0.1,
    62539,
    3
);
ctc_parameter!(
    HEATPUMP_DEFROST_TIMER,
    62137,
    "Heat pump 1 (A1): Defrost timer",
    1.0,
    62539,
    13
);
ctc_parameter!(
    HEATPUMP_OUTDOOR_TEMP,
    62147,
    "Heat pump 1 (A1): Outdoor temp",
    0.1,
    62540,
    7
);
ctc_parameter!(
    HEATPUMP_SOFTWARE_VERSION,
    62157,
    "Heat pump 1 (A1): Software version",
    1.0,
    62541,
    1
);
ctc_parameter!(
    HEATPUMP_CURRENT_RPS,
    62193,
    "Heat pump 1 (A1): Current RPS",
    0.1,
    62543,
    5
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

// region: --- CTC Diagnostic Modbus Parameters

ctc_parameter!(
    CTC_DAYS_FILTER_MAINTENANCE,
    62283,
    "Days until next filter maintenance",
    1.0,
    62548,
    15
);

/// Transfer alarm/info reference into text buffer (write-only)
/// Values 0-9999: Alarm number (0 = Alarm number 0)
/// Values 10000-19999: Info number (10000 = Info number 0)
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
                &HEATSYSTEM_FLOW_NIGHT_REDUCTION,
                &HEATSYSTEM_OUTDOOR_NIGHT_REDUCTION,
                &HEATSYSTEM_ALARM_LOW_ROOM_TEMP,
                &HEATSYSTEM_HOLIDAY_REDUCTION,
                &HEATSYSTEM_FLOW_HOLIDAY_REDUCTION,
                &HEATSYSTEM_FLOW_SETPOINT,
                &HEATSYSTEM_FLOW_TEMP,
                &HEATSYSTEM_STATUS,
                // Common parameters
                &CTC_RETURN_TEMP,
                &CTC_HOT_WATER_MODE,
                &CTC_ROOM_TEMP,
                &CTC_OUTDOOR_TEMP,
                &CTC_VACCATION_DAYS,
                // Heat Pump parameters
                &HEATPUMP_BLOCKED,
                &HEATPUMP_MAX_RMP,
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
                &HEATPUMP_FAN_SPEED,
                &HEATPUMP_DEFROST_TIMER,
                &HEATPUMP_OUTDOOR_TEMP,
                &HEATPUMP_SOFTWARE_VERSION,
                &HEATPUMP_CURRENT_RPS,
                &HEATPUMP_TYPE,
                &HEATPUMP_COMPRESSOR_MODEL,
                // Diagnostic parameters
                &CTC_DAYS_FILTER_MAINTENANCE,
                // Text buffer parameters
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
        visible: 62500,
        bit: 0,
        factor: factor.unwrap_or(1.0),
        description: "Custom CTC Parameter",
    }
}
