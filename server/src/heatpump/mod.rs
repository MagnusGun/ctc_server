//! Heat pump statistics tracking module
//!
//! Provides compressor cycle statistics including:
//! - Cycle times (min/max/avg)
//! - Compressor starts per time window (hour/day/week/month/year)
//! - Operating hours per time window
//! - Outdoor temperature correlation

pub mod poller;
pub mod stats;
pub mod step_detector;

pub use stats::HeatPumpStats;
