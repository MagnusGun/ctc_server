//! Energy management modules
//!
//! This module contains components for energy consumption tracking,
//! tariff calculations, and external energy service integrations.

pub mod elpris;
pub mod grid;
pub mod price;
pub mod tariff;
pub mod tibber;

// Re-export commonly used types
pub use grid::GridState;
pub use price::PriceState;
