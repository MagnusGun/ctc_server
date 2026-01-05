//! `SmartGrid` control modes
//!
//! Defines the four `SmartGrid` operating modes that can be set via GPIO terminals
//! connected to the CTC heat pump's external smart grid inputs (K24/K25).

/// `SmartGrid` control modes
///
/// These modes control how the heat pump responds to external grid signals,
/// allowing integration with dynamic electricity pricing and grid capacity management.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartGridMode {
    /// Normal operation - no external grid signal applied
    Normal,
    /// Blocking - stop heating/cooling to reduce grid load
    /// Equivalent to power-save mode without exhausting config register write cycles
    Blocking,
    /// Low Price - prioritize operation when energy is cheap
    LowPrice,
    /// Overcapacity - maximum operation when excess energy available
    Overcapacity,
}

impl SmartGridMode {
    /// Get required K24/K25 terminal states for this mode
    ///
    /// Returns (`k24_closed`, `k25_closed`)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_states() {
        assert_eq!(SmartGridMode::Normal.terminal_states(), (false, false));
        assert_eq!(SmartGridMode::Blocking.terminal_states(), (true, false));
        assert_eq!(SmartGridMode::LowPrice.terminal_states(), (false, true));
        assert_eq!(SmartGridMode::Overcapacity.terminal_states(), (true, true));
    }

    #[test]
    fn test_display() {
        assert_eq!(SmartGridMode::Normal.to_string(), "Normal");
        assert_eq!(SmartGridMode::Blocking.to_string(), "Blocking");
        assert_eq!(SmartGridMode::LowPrice.to_string(), "LowPrice");
        assert_eq!(SmartGridMode::Overcapacity.to_string(), "Overcapacity");
    }

    #[test]
    fn test_from_str() {
        assert_eq!("normal".parse::<SmartGridMode>(), Ok(SmartGridMode::Normal));
        assert_eq!(
            "blocking".parse::<SmartGridMode>(),
            Ok(SmartGridMode::Blocking)
        );
        assert_eq!(
            "lowprice".parse::<SmartGridMode>(),
            Ok(SmartGridMode::LowPrice)
        );
        assert_eq!(
            "low_price".parse::<SmartGridMode>(),
            Ok(SmartGridMode::LowPrice)
        );
        assert_eq!(
            "overcapacity".parse::<SmartGridMode>(),
            Ok(SmartGridMode::Overcapacity)
        );
        assert!("invalid".parse::<SmartGridMode>().is_err());
    }
}
