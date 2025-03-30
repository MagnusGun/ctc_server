#![allow(dead_code)]
pub mod bms_parameters;
use tokio::sync::MutexGuard;
use tokio_modbus::client::{Context, Reader, Writer};

// region: --- Modbus Parameter Struct
// Define the access type.
#[derive(Debug, PartialEq)]
pub enum Access {
    R,
    RW,
}

#[derive(Debug)]
pub struct ModbusParameter {
    /// Logical register identifier for this parameter (e.g., 61509 for room temperature)
    pub id: u16,
    /// Indicates whether the parameter's value is signed (can be negative).
    pub signed: bool,
    /// Access type: either read-only (R) or read/write (RW).
    pub access: Access,
    /// Register address from the "Max" column.
    /// Typically holds a value like the maximum setpoint or the primary value used for configuration.
    pub reg_max: u16,
    /// Register address from the "Min" column.
    /// Often represents the minimum allowed value or an associated lower bound.
    pub reg_min: u16,
    /// Register address from the "Step" column.
    /// Indicates the increment (or resolution) used when adjusting the parameter.
    pub reg_step: u16,
    /// Register address from the "Visible" column.
    /// This register contains a bit field (mask) indicating which parameters are supported or active.
    pub visible: u16,
    /// The bit position within the bit field from the "Visible" register corresponding to this parameter.
    pub bit: u8,
    /// Scaling factor to convert the raw register value into physical units.
    /// For example, a factor of 0.1 converts a raw value of 221 into 22.1
    pub factor: f32,
}

impl ModbusParameter {
    /// Returns the scaled value for a given raw register value.
    pub fn get_scaled_value_vector(&self, value: Vec<u16>) -> Vec<f32> {
        if self.signed {
            value.iter().map(|v| {(*v as i16 as f32 * self.factor * 10.0).round() / 10.0}).collect()
        } else {
            value.iter().map(|v| {(*v as u16 as f32 * self.factor * 10.0).round() / 10.0}).collect() 
        }
    }

    pub fn to_scaled_value_vector(&self, value: f32) -> Vec<u16> {
        vec![(value / self.factor).round() as u16]
    }

    pub async fn read(&self, mut ctx: MutexGuard<'_, Context>) -> Result<f32, Box<dyn std::error::Error>> {
        match ctx.read_holding_registers(self.id, 1).await {
            Ok(rsp) => {match rsp {
                Ok(rsp) => {
                    let rsp = self.get_scaled_value_vector(rsp);
                    match rsp.first() {
                        Some(value) => return Ok(*value),
                        None => return Err(Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "Empty response"))),
                    }
                },
                Err(e) => return Err(Box::new(e)),
            }}
            Err(e) => return Err(Box::new(e)),
        }
    }

    pub async fn write(&self, mut ctx: MutexGuard<'_, Context>, value: f32) -> Result<(), Box<dyn std::error::Error>> {
        if self.access == Access::R {
            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Read-only parameter")));
        }
        let scaled_value = self.to_scaled_value_vector(value);
        match ctx.write_multiple_registers(self.id, &scaled_value).await {
            Ok(_) => Ok(()),
            Err(e) => Err(Box::new(e)),   
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

impl HotWaterMode {
    pub fn to_u16(&self) -> u16 {
        match self {
            HotWaterMode::Economy => 0,
            HotWaterMode::Normal => 1,
            HotWaterMode::Comfort => 2,
            HotWaterMode::Manual => 3,
        }
    }

    pub fn from_u16(value: u16) -> Option<HotWaterMode> {
        match value {
            0 => Some(HotWaterMode::Economy),
            1 => Some(HotWaterMode::Normal),
            2 => Some(HotWaterMode::Comfort),
            3 => Some(HotWaterMode::Manual),
            _ => None,
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

impl HeatSystemStatus {
    pub fn to_u16(&self) -> u16 {
        match self {
            HeatSystemStatus::HeatingOff => 0,
            HeatSystemStatus::Vacation => 1,
            HeatSystemStatus::NightReduction => 2,
            HeatSystemStatus::On => 3,
        }
    }

    pub fn from_u16(value: u16) -> Option<HeatSystemStatus> {
        match value {
            0 => Some(HeatSystemStatus::HeatingOff),
            1 => Some(HeatSystemStatus::Vacation),
            2 => Some(HeatSystemStatus::NightReduction),
            3 => Some(HeatSystemStatus::On),
            _ => None,
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

// region: --- Unit tests

// unit tests
#[cfg(test)]
mod tests {
    use crate::modbus::{bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, HEATSYSTEM_STATUS}, HeatSystemStatus, HotWaterMode};

    #[test]
    fn test_get_scaled_value_vector_0_1() {
        let raw_values = vec![221, 222, 223];
        let scaled_values = HEATSYSTEM_ROOM_SETTEMP.get_scaled_value_vector(raw_values);
        assert_eq!(scaled_values, vec![22.1, 22.2, 22.3]);
    }

    #[test]
    fn test_get_scaled_value_vector_1_0() {
        let raw_values = vec![22, 23, 24];
        let scaled_values = HEATSYSTEM_STATUS.get_scaled_value_vector(raw_values);
        assert_eq!(scaled_values, vec![22.0, 23.0, 24.0]);
    }



    #[test]
    fn test_to_scaled_value_vector_0_1() {
        let value = 22.1;
        let scaled_value = HEATSYSTEM_ROOM_SETTEMP.to_scaled_value_vector(value);
        assert_eq!(scaled_value, vec![221]);
    }

    #[test]
    fn test_to_scaled_value_vector_1_0() {
        let value = 22_f32;
        let scaled_value = HEATSYSTEM_STATUS.to_scaled_value_vector(value);
        assert_eq!(scaled_value, vec![22]);
    }

    #[test]
    fn test_hot_water_mode_to_u16() {
        assert_eq!(HotWaterMode::Economy.to_u16(), 0);
        assert_eq!(HotWaterMode::Normal.to_u16(), 1);
        assert_eq!(HotWaterMode::Comfort.to_u16(), 2);
        assert_eq!(HotWaterMode::Manual.to_u16(), 3);
    }

    #[test]
    fn test_hot_water_mode_from_u16() {
        assert_eq!(HotWaterMode::from_u16(0), Some(HotWaterMode::Economy));
        assert_eq!(HotWaterMode::from_u16(1), Some(HotWaterMode::Normal));
        assert_eq!(HotWaterMode::from_u16(2), Some(HotWaterMode::Comfort));
        assert_eq!(HotWaterMode::from_u16(3), Some(HotWaterMode::Manual));
        assert_eq!(HotWaterMode::from_u16(4), None);
    }

    #[test]
    fn test_heat_system_status_to_u16() {
        assert_eq!(HeatSystemStatus::HeatingOff.to_u16(), 0);
        assert_eq!(HeatSystemStatus::Vacation.to_u16(), 1);
        assert_eq!(HeatSystemStatus::NightReduction.to_u16(), 2);
        assert_eq!(HeatSystemStatus::On.to_u16(), 3);
    }

    #[test]
    fn test_heat_system_status_from_u16() {
        assert_eq!(HeatSystemStatus::from_u16(0), Some(HeatSystemStatus::HeatingOff));
        assert_eq!(HeatSystemStatus::from_u16(1), Some(HeatSystemStatus::Vacation));
        assert_eq!(HeatSystemStatus::from_u16(2), Some(HeatSystemStatus::NightReduction));
        assert_eq!(HeatSystemStatus::from_u16(3), Some(HeatSystemStatus::On));
        assert_eq!(HeatSystemStatus::from_u16(4), None);
    }
// endregion: --- Unit tests
}