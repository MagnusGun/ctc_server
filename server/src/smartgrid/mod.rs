//! `SmartGrid` control modules
//!
//! This module contains components for controlling the CTC heat pump's
//! `SmartGrid` functionality via GPIO relay outputs.

pub mod gpio;
pub mod mode;
pub mod scheduler;

// Re-export commonly used types
pub use gpio::GpioController;
pub use mode::SmartGridMode;
pub use scheduler::apply_mode;
