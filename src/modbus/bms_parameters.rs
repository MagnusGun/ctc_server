// Define the modbus parameters for the CTC Heating System 1.
// see 'Service document-BMS Register-17003548.pdf'
#![allow(dead_code)]
use crate::modbus::{Access, ModbusParameter};

// region: --- CTC Heating System 1 Modbus Parameters

pub const HEATSYSTEM_ROOM_SETTEMP: ModbusParameter = ModbusParameter {
    id: 61509,
    signed: true,
    access: Access::RW,
    reg_max: 60027,
    reg_min: 60028,
    reg_step: 60029,
    visible: 62500,
    bit: 9,
    factor: 0.1,
};

pub const HEATSYSTEM_INCLINATION: ModbusParameter = ModbusParameter {
    id: 61513,
    signed: true,
    access: Access::RW,
    reg_max: 60039,
    reg_min: 60040,
    reg_step: 60041,
    visible: 62500,
    bit: 13,
    factor: 0.1,
};

pub const HEATSYSTEM_ADJUSTMENT: ModbusParameter = ModbusParameter {
    id: 61517,
    signed: true,
    access: Access::RW,
    reg_max: 60051,
    reg_min: 60052,
    reg_step: 60053,
    visible: 62501,
    bit: 1,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_MAX_TEMP: ModbusParameter = ModbusParameter {
    id: 61534,
    signed: true,
    access: Access::RW,
    reg_max: 60102,
    reg_min: 60103,
    reg_step: 60104,
    visible: 62502,
    bit: 2,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_MIN_TEMP: ModbusParameter = ModbusParameter {
    id: 61538,
    signed: true,
    access: Access::RW,
    reg_max: 60114,
    reg_min: 60115,
    reg_step: 60116,
    visible: 62502,
    bit: 6,
    factor: 0.1,
};

pub const HEATSYSTEM_HEATING_MODE: ModbusParameter = ModbusParameter {
    id: 61542,
    signed: true,
    access: Access::RW,
    reg_max: 60126,
    reg_min: 60127,
    reg_step: 60128,
    visible: 62502,
    bit: 10,
    factor: 1.0,
};

pub const HEATSYSTEM_HEAT_OFF_TEMP: ModbusParameter = ModbusParameter {
    id: 61546,
    signed: true,
    access: Access::RW,
    reg_max: 60138,
    reg_min: 60139,
    reg_step: 60140,
    visible: 62502,
    bit: 14,
    factor: 0.1,
};

pub const HEATSYSTEM_HEAT_OFF_TIME: ModbusParameter = ModbusParameter {
    id: 61550,
    signed: true,
    access: Access::RW,
    reg_max: 60150,
    reg_min: 60151,
    reg_step: 60152,
    visible: 62503,
    bit: 2,
    factor: 1.0,
};

pub const HEATSYSTEM_ROOM_TEMP_NIGHT_REDUCTION: ModbusParameter = ModbusParameter {
    id: 61554,
    signed: true,
    access: Access::RW,
    reg_max: 60162,
    reg_min: 60163,
    reg_step: 60164,
    visible: 62503,
    bit: 6,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_NIGHT_REDUCTION: ModbusParameter = ModbusParameter {
    id: 61558,
    signed: true,
    access: Access::RW,
    reg_max: 60174,
    reg_min: 60175,
    reg_step: 60176,
    visible: 62503,
    bit: 10,
    factor: 0.1,
};

pub const HEATSYSTEM_OUTDOOR_NIGHT_REDUCTION: ModbusParameter = ModbusParameter {
    id: 61562,
    signed: true,
    access: Access::RW,
    reg_max: 60186,
    reg_min: 60187,
    reg_step: 60188,
    visible: 62503,
    bit: 14,
    factor: 0.1,
};

pub const HEATSYSTEM_ALARM_LOW_ROOM_TEMP: ModbusParameter = ModbusParameter {
    id: 61566,
    signed: true,
    access: Access::RW,
    reg_max: 60198,
    reg_min: 60199,
    reg_step: 60200,
    visible: 62504,
    bit: 2,
    factor: 0.1,
};

pub const HEATSYSTEM_HOLIDAY_REDUCTION: ModbusParameter = ModbusParameter {
    id: 61602,
    signed: true,
    access: Access::RW,
    reg_max: 60306,
    reg_min: 60307,
    reg_step: 60308,
    visible: 62506,
    bit: 6,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_HOLIDAY_REDUCTION: ModbusParameter = ModbusParameter {
    id: 61606,
    signed: true,
    access: Access::RW,
    reg_max: 60318,
    reg_min: 60319,
    reg_step: 60320,
    visible: 62506,
    bit: 10,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_SETPOINT: ModbusParameter = ModbusParameter {
    id: 62007,
    signed: true,
    access: Access::R,
    // Not used for read-only parameters, so max, min, and step are 0.
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 11,
    factor: 0.1,
};

pub const HEATSYSTEM_FLOW_TEMP: ModbusParameter = ModbusParameter {
    id: 62011,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 15,
    factor: 0.1,
};

pub const HEATSYSTEM_STATUS: ModbusParameter = ModbusParameter {
    id: 62246,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62546,
    bit: 10,
    factor: 1.0,
};

// endregion: --- CTC Heating System 1 Modbus Parameters

// region: --- CTC Common Modbus Parameters
pub const CTC_RETURN_TEMP: ModbusParameter = ModbusParameter {
    id: 62015,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 3,
    factor: 0.1,
};

pub const CTC_HOT_WATER_MODE: ModbusParameter = ModbusParameter {
    id: 61500,
    signed: true,
    access: Access::RW,
    reg_max: 60000,
    reg_min: 60001,
    reg_step: 60002,
    visible: 62500,
    bit: 0,
    factor: 1.0,
};

pub const CTC_ROOM_TEMP: ModbusParameter = ModbusParameter {
    id: 62203,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62543,
    bit: 15,
    factor: 0.1,
};

pub const CTC_OUTDOOR_TEMP: ModbusParameter = ModbusParameter {
    id: 62000,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 4,
    factor: 0.1,
};

pub const CTC_VACCATION_DAYS: ModbusParameter = ModbusParameter {
    id: 61508,
    signed: true,
    access: Access::RW,
    reg_max: 60024,
    reg_min: 60025,
    reg_step: 60026,
    visible: 62500,
    bit: 8,
    factor: 1.0,
};


// endregion: --- CTC Common Modbus Parameters

// region: --- CTC HeatPump 1 Modbus Parameters
pub const HEATPUMP_BLOCKED: ModbusParameter = ModbusParameter {
    id: 61521,
    signed: true,
    access: Access::RW,
    reg_max: 60063,
    reg_min: 60064,
    reg_step: 60065,
    visible: 62501,
    bit: 5,
    factor: 1.0,
};

pub const HEATPUMP_MAX_RMP: ModbusParameter = ModbusParameter {
    id: 61572,
    signed: true,
    access: Access::RW,
    reg_max: 60216,
    reg_min: 60217,
    reg_step: 60218,
    visible: 62504,
    bit: 8,
    factor: 0.1,
};

pub const HEATPUMP_STATUS: ModbusParameter = ModbusParameter {
    id: 62017,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 5,
    factor: 1.0,
};

pub const HEATPUMP_INLET_TEMP: ModbusParameter = ModbusParameter {
    id: 62027,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 15,
    factor: 0.1,
};

pub const HEATPUMP_OUTLET_TEMP: ModbusParameter = ModbusParameter {
    id: 62037,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62533,
    bit: 9,
    factor: 0.1,
};

pub const HEATPUMP_DISCHARGE_TEMP: ModbusParameter = ModbusParameter {
    id: 62047,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62534,
    bit: 3,
    factor: 0.1,
};

pub const HEATPUMP_SUCTION_TEMP: ModbusParameter = ModbusParameter {
    id: 62057,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62534,
    bit: 13,
    factor: 0.1,
};

pub const HEATPUMP_HIGH_PRESSURE: ModbusParameter = ModbusParameter {
    id: 62067,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62535,
    bit: 7,
    factor: 0.1,
};

pub const HEATPUMP_LOW_PRESSURE: ModbusParameter = ModbusParameter {
    id: 62077,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62536,
    bit: 1,
    factor: 0.1,
};

pub const HEATPUMP_BRINE_INLET_TEMP: ModbusParameter = ModbusParameter {
    id: 62087,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62536,
    bit: 11,
    factor: 0.1,
};

pub const HEATPUMP_BRINE_OUTLET_TEMP: ModbusParameter = ModbusParameter {
    id: 62097,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62537,
    bit: 5,
    factor: 0.1,
};

pub const HEATPUMP_CHARGE_PUMP: ModbusParameter = ModbusParameter {
    id: 62107,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62537,
    bit: 15,
    factor: 0.1,
};

pub const HEATPUMP_BRINE_PUMP: ModbusParameter = ModbusParameter {
    id: 62117,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62538,
    bit: 9,
    factor: 0.1,
};

pub const HEATPUMP_FAN_SPEED: ModbusParameter = ModbusParameter {
    id: 62127,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62539,
    bit: 3,
    factor: 0.1,
};

pub const HEATPUMP_DEFROST_TIMER: ModbusParameter = ModbusParameter {
    id: 62137,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62539,
    bit: 13,
    factor: 1.0,
};

pub const HEATPUMP_OUTDOOR_TEMP: ModbusParameter = ModbusParameter {
    id: 62147,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62540,
    bit: 7,
    factor: 0.1,
};

pub const HEATPUMP_SOFTWARE_VERSION: ModbusParameter = ModbusParameter {
    id: 62157,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62541,
    bit: 1,
    factor: 1.0,
};

pub const HEATPUMP_CURRENT_RPS: ModbusParameter = ModbusParameter {
    id: 62193,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62543,
    bit: 5,
    factor: 0.1,
};

pub const HEATPUMP_TYPE: ModbusParameter = ModbusParameter {
    id: 62254,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62547,
    bit: 2,
    factor: 1.0,
};

pub const HEATPUMP_COMPRESSOR_MODEL: ModbusParameter = ModbusParameter {
    id: 62264,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62547,
    bit: 12,
    factor: 1.0,
};
// endregion: --- CTC HeatPump 1 Modbus Parameters