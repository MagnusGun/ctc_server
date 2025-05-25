// Define the modbus parameters for the CTC Heating System 1.
// see 'Service document-BMS Register-17003548.pdf'
#![allow(dead_code)]
use crate::modbus::{Access, CTCModbusParameter};

// region: --- CTC Heating System 1 Modbus Parameters

pub static HEATSYSTEM_ROOM_SETTEMP: CTCModbusParameter = CTCModbusParameter {
    id: 61509,
    signed: true,
    access: Access::RW,
    reg_max: 60027,
    reg_min: 60028,
    reg_step: 60029,
    visible: 62500,
    bit: 9,
    factor: 0.1,
    description: "Heating system 1: Set room temperature",
    // description: String::from("Heating system 1: Setting room temp"),
};

pub static HEATSYSTEM_INCLINATION: CTCModbusParameter = CTCModbusParameter {
    id: 61513,
    signed: true,
    access: Access::RW,
    reg_max: 60039,
    reg_min: 60040,
    reg_step: 60041,
    visible: 62500,
    bit: 13,
    factor: 0.1,
    description: "Heating system 1: Change inclination",
};

pub static HEATSYSTEM_ADJUSTMENT: CTCModbusParameter = CTCModbusParameter {
    id: 61517,
    signed: true,
    access: Access::RW,
    reg_max: 60051,
    reg_min: 60052,
    reg_step: 60053,
    visible: 62501,
    bit: 1,
    factor: 0.1,
    description: "Heating system 1: Change adjustment",
};

pub static HEATSYSTEM_FLOW_MAX_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 61534,
    signed: true,
    access: Access::RW,
    reg_max: 60102,
    reg_min: 60103,
    reg_step: 60104,
    visible: 62502,
    bit: 2,
    factor: 0.1,
    description: "Heating system 1: Max Primary flow °C",
};

pub static HEATSYSTEM_FLOW_MIN_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 61538,
    signed: true,
    access: Access::RW,
    reg_max: 60114,
    reg_min: 60115,
    reg_step: 60116,
    visible: 62502,
    bit: 6,
    factor: 0.1,
    description: "Heating system 1: Min primary flow °C",
};

pub static HEATSYSTEM_HEATING_MODE: CTCModbusParameter = CTCModbusParameter {
    id: 61542,
    signed: true,
    access: Access::RW,
    reg_max: 60126,
    reg_min: 60127,
    reg_step: 60128,
    visible: 62502,
    bit: 10,
    factor: 1.0,
    description: "Heating system 1: Heating mode",
};

pub static HEATSYSTEM_HEAT_OFF_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 61546,
    signed: true,
    access: Access::RW,
    reg_max: 60138,
    reg_min: 60139,
    reg_step: 60140,
    visible: 62502,
    bit: 14,
    factor: 0.1,
    description: "Heating system 1: Heating off, out °C",
};

pub static HEATSYSTEM_HEAT_OFF_TIME: CTCModbusParameter = CTCModbusParameter {
    id: 61550,
    signed: true,
    access: Access::RW,
    reg_max: 60150,
    reg_min: 60151,
    reg_step: 60152,
    visible: 62503,
    bit: 2,
    factor: 1.0,
    description: "Heating system 1: Heating off time",
};

pub static HEATSYSTEM_ROOM_TEMP_NIGHT_REDUCTION: CTCModbusParameter = CTCModbusParameter {
    id: 61554,
    signed: true,
    access: Access::RW,
    reg_max: 60162,
    reg_min: 60163,
    reg_step: 60164,
    visible: 62503,
    bit: 6,
    factor: 0.1,
    description: "Heating system 1: Room temp night reduction",
};

pub static HEATSYSTEM_FLOW_NIGHT_REDUCTION: CTCModbusParameter = CTCModbusParameter {
    id: 61558,
    signed: true,
    access: Access::RW,
    reg_max: 60174,
    reg_min: 60175,
    reg_step: 60176,
    visible: 62503,
    bit: 10,
    factor: 0.1,
    description: "Heating system 1: Primary flow Night reduction",
};

pub static HEATSYSTEM_OUTDOOR_NIGHT_REDUCTION: CTCModbusParameter = CTCModbusParameter {
    id: 61562,
    signed: true,
    access: Access::RW,
    reg_max: 60186,
    reg_min: 60187,
    reg_step: 60188,
    visible: 62503,
    bit: 14,
    factor: 0.1,
    description: "Heating system 1: Outdoor temp night reduction",
};

pub static HEATSYSTEM_ALARM_LOW_ROOM_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 61566,
    signed: true,
    access: Access::RW,
    reg_max: 60198,
    reg_min: 60199,
    reg_step: 60200,
    visible: 62504,
    bit: 2,
    factor: 0.1,
    description: "Heating system 1: Alarm low room temperature",
};

pub static HEATSYSTEM_HOLIDAY_REDUCTION: CTCModbusParameter = CTCModbusParameter {
    id: 61602,
    signed: true,
    access: Access::RW,
    reg_max: 60306,
    reg_min: 60307,
    reg_step: 60308,
    visible: 62506,
    bit: 6,
    factor: 0.1,
    description: "Heating system 1: Holiday reduction",
};

pub static HEATSYSTEM_FLOW_HOLIDAY_REDUCTION: CTCModbusParameter = CTCModbusParameter {
    id: 61606,
    signed: true,
    access: Access::RW,
    reg_max: 60318,
    reg_min: 60319,
    reg_step: 60320,
    visible: 62506,
    bit: 10,
    factor: 0.1,
    description: "Heating system 1: Primary flow Holiday reduction",
};

pub static HEATSYSTEM_FLOW_SETPOINT: CTCModbusParameter = CTCModbusParameter {
    id: 62007,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 11,
    factor: 0.1,
    description: "Heating system 1: Temperature setpoint primary flow",
};

pub static HEATSYSTEM_FLOW_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62011,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 15,
    factor: 0.1,
    description: "Heating system 1: Primary flow temperature",
};

pub static HEATSYSTEM_STATUS: CTCModbusParameter = CTCModbusParameter {
    id: 62246,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62546,
    bit: 10,
    factor: 1.0,
    description: "Heating system 1 status",
};

// endregion: --- CTC Heating System 1 Modbus Parameters

// region: --- CTC Common Modbus Parameters

pub static CTC_RETURN_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62015,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 3,
    factor: 0.1,
    description: "Return temp",
};

pub static CTC_HOT_WATER_MODE: CTCModbusParameter = CTCModbusParameter {
    id: 61500,
    signed: true,
    access: Access::RW,
    reg_max: 60000,
    reg_min: 60001,
    reg_step: 60002,
    visible: 62500,
    bit: 0,
    factor: 1.0,
    description: "Hot water mode",
};

pub static CTC_ROOM_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62203,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62543,
    bit: 15,
    factor: 0.1,
    description: "Current room temp 1",
};

pub static CTC_OUTDOOR_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62000,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62531,
    bit: 4,
    factor: 0.1,
    description: "Outdoor temperature",
};

pub static CTC_VACCATION_DAYS: CTCModbusParameter = CTCModbusParameter {
    id: 61508,
    signed: true,
    access: Access::RW,
    reg_max: 60024,
    reg_min: 60025,
    reg_step: 60026,
    visible: 62500,
    bit: 8,
    factor: 1.0,
    description: "Number of vacation days timer",
};

// endregion: --- CTC Common Modbus Parameters

// region: --- CTC HeatPump 1 Modbus Parameters

pub static HEATPUMP_BLOCKED: CTCModbusParameter = CTCModbusParameter {
    id: 61521,
    signed: true,
    access: Access::RW,
    reg_max: 60063,
    reg_min: 60064,
    reg_step: 60065,
    visible: 62501,
    bit: 5,
    factor: 1.0,
    description: "Heat pump 1 (A1): Blocked",
};

pub static HEATPUMP_MAX_RMP: CTCModbusParameter = CTCModbusParameter {
    id: 61572,
    signed: true,
    access: Access::RW,
    reg_max: 60216,
    reg_min: 60217,
    reg_step: 60218,
    visible: 62504,
    bit: 8,
    factor: 0.1,
    description: "Heat pump 1 (A1): Max RPS",
};

pub static HEATPUMP_STATUS: CTCModbusParameter = CTCModbusParameter {
    id: 62017,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 5,
    factor: 1.0,
    description: "Heat pump 1 (A1): Status",
};

pub static HEATPUMP_INLET_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62027,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62532,
    bit: 15,
    factor: 0.1,
    description: "Heat pump 1 (A1) HP in",
};

pub static HEATPUMP_OUTLET_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62037,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62533,
    bit: 9,
    factor: 0.1,
    description: "Heat pump 1 (A1) HP out",
};

pub static HEATPUMP_DISCHARGE_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62047,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62534,
    bit: 3,
    factor: 0.1,
    description: "Heat pump 1 (A1): Discharge temperature",
};

pub static HEATPUMP_SUCTION_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62057,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62534,
    bit: 13,
    factor: 0.1,
    description: "Heat pump 1 (A1): Suction gas temperature",
};

pub static HEATPUMP_HIGH_PRESSURE: CTCModbusParameter = CTCModbusParameter {
    id: 62067,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62535,
    bit: 7,
    factor: 0.1,
    description: "Heat pump 1 (A1): High pressure",
};

pub static HEATPUMP_LOW_PRESSURE: CTCModbusParameter = CTCModbusParameter {
    id: 62077,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62536,
    bit: 1,
    factor: 0.1,
    description: "Heat pump 1 (A1): Low Pressure",
};

pub static HEATPUMP_BRINE_INLET_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62087,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62536,
    bit: 11,
    factor: 0.1,
    description: "Heat pump 1 (A1): Brine in",
};

pub static HEATPUMP_BRINE_OUTLET_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62097,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62537,
    bit: 5,
    factor: 0.1,
    description: "Heat pump 1 (A1): Brine out",
};

pub static HEATPUMP_CHARGE_PUMP: CTCModbusParameter = CTCModbusParameter {
    id: 62107,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62537,
    bit: 15,
    factor: 0.1,
    description: "Heat pump 1 (A1): Charge pump",
};

pub static HEATPUMP_BRINE_PUMP: CTCModbusParameter = CTCModbusParameter {
    id: 62117,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62538,
    bit: 9,
    factor: 0.1,
    description: "Heat pump 1 (A1): Brine pump",
};

pub static HEATPUMP_FAN_SPEED: CTCModbusParameter = CTCModbusParameter {
    id: 62127,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62539,
    bit: 3,
    factor: 0.1,
    description: "Heat pump 1 (A1): Fan",
};

pub static HEATPUMP_DEFROST_TIMER: CTCModbusParameter = CTCModbusParameter {
    id: 62137,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62539,
    bit: 13,
    factor: 1.0,
    description: "Heat pump 1 (A1): Defrost timer",
};

pub static HEATPUMP_OUTDOOR_TEMP: CTCModbusParameter = CTCModbusParameter {
    id: 62147,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62540,
    bit: 7,
    factor: 0.1,
    description: "Heat pump 1 (A1): Outdoor temp",
};

pub static HEATPUMP_SOFTWARE_VERSION: CTCModbusParameter = CTCModbusParameter {
    id: 62157,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62541,
    bit: 1,
    factor: 1.0,
    description: "Heat pump 1 (A1): Software version",
};

pub static HEATPUMP_CURRENT_RPS: CTCModbusParameter = CTCModbusParameter {
    id: 62193,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62543,
    bit: 5,
    factor: 0.1,
    description: "Heat pump 1 (A1): Current RPS",
};

pub static HEATPUMP_TYPE: CTCModbusParameter = CTCModbusParameter {
    id: 62254,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62547,
    bit: 2,
    factor: 1.0,
    description: "Heat pump 1 (A1) Type",
};

pub static HEATPUMP_COMPRESSOR_MODEL: CTCModbusParameter = CTCModbusParameter {
    id: 62264,
    signed: true,
    access: Access::R,
    reg_max: 0,
    reg_min: 0,
    reg_step: 0,
    visible: 62547,
    bit: 12,
    factor: 1.0,
    description: "Heat pump 1 (A1) compressor model",
};

// endregion: --- CTC HeatPump 1 Modbus Parameters