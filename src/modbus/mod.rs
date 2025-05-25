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
pub struct CTCModbusParameter {
    /// This register address contains the value for the parameter.
    pub id: u16,
    /// Indicates whether the parameter's value is signed or unsigned.
    pub signed: bool,
    /// Access type: either read-only (R) or read/write (RW).
    pub access: Access,
    /// This register address contains the maximum value for the parameter.
    pub reg_max: u16,
    /// This register address contains the minimum value for the parameter.
    pub reg_min: u16,
    /// This register address contains the step size for the parameter e.g., 0.1 for temperature.
    pub reg_step: u16,
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
    /// Returns the scaled value for a given raw register value.
    #[must_use]
    pub fn get_scaled_value_vector(&self, value: &[u16]) -> Vec<f32> {
        if self.signed {
            value.iter().map(|v| {(f32::from(*v as i16) * self.factor * 10.0).round() / 10.0}).collect()
        } else {
            value.iter().map(|v| {(f32::from(*v) * self.factor * 10.0).round() / 10.0}).collect() 
        }
    }

    #[must_use]
    pub fn to_scaled_value_vector(&self, value: f32) -> Vec<u16> {
        vec![(value / self.factor).round() as u16]
    }

    /// Reads a value from a Modbus register and returns it as a scaled value.
    /// 
    /// # Arguments
    /// * `ctx` - The Modbus context to use for the read operation.
    /// 
    /// # Returns
    /// * The scaled value read from the register.
    /// 
    /// # Errors
    /// * Returns an error if the read operation fails.
    /// * Returns an error if the response is empty.
    pub async fn read(&self, mut ctx: MutexGuard<'_, Context>) -> Result<f32, Box<dyn std::error::Error>> {
        let result = ctx.read_holding_registers(self.id, 1).await?;
        let raw_values = result?;
        let scaled_values = self.get_scaled_value_vector(&raw_values);
        
        scaled_values.first()
            .copied()
            .ok_or_else(|| Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData, 
                "Empty response"
            )) as Box<dyn std::error::Error>)
    }

    /// Writes a scaled value to the Modbus register.
    /// This is the inverse of `get_scaled_value_vector`.
    /// For example, if the scaled value is 22.1 and the factor is 0.1,
    /// the raw value would be 221.
    /// This function is used for writing values to the Modbus register.
    /// # Arguments
    /// * `ctx` - The Modbus context to use for the write operation.
    /// * `value` - The scaled value to write to the Modbus register.
    /// # Returns
    /// * A Result indicating success or failure of the write operation.
    /// # ERRORS
    /// * Returns an error if the value is not a valid scaled value for this parameter.
    /// * Returns an error if the write operation fails.
    /// * Returns an error if the parameter is read-only.
    pub async fn write(&self, mut ctx: MutexGuard<'_, Context>, value: f32) -> Result<(), Box<dyn std::error::Error>> {
        if self.access == Access::R {
            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Read-only parameter")));
        }
        let scaled_value = self.to_scaled_value_vector(value);
        ctx.write_multiple_registers(self.id, &scaled_value).await?;
        Ok(())
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
            HotWaterMode::Normal  => write!(f, "Normal"),
            HotWaterMode::Comfort => write!(f, "Comfort"),
            HotWaterMode::Manual  => write!(f, "Manual"),
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

// region: --- Unit tests
#[cfg(test)]
mod tests {
    use crate::modbus::{bms_parameters::{HEATSYSTEM_ROOM_SETTEMP, HEATSYSTEM_STATUS}, HeatSystemStatus, HotWaterMode};

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
        assert_eq!(HeatSystemStatus::try_from(0), Ok(HeatSystemStatus::HeatingOff));
        assert_eq!(HeatSystemStatus::try_from(1), Ok(HeatSystemStatus::Vacation));
        assert_eq!(HeatSystemStatus::try_from(2), Ok(HeatSystemStatus::NightReduction));
        assert_eq!(HeatSystemStatus::try_from(3), Ok(HeatSystemStatus::On));
        assert!(HeatSystemStatus::try_from(4).is_err());
    }
}
// endregion: --- Unit tests