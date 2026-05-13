//! `SmartGrid` control modules
//!
//! This module contains components for controlling the CTC heat pump's
//! `SmartGrid` functionality via GPIO relay outputs.
//!
//! Public surface is the [`SmartGridHandle`] (a cheap-clone mpsc sender)
//! returned by [`actor::spawn`]. Route handlers send commands; the actor
//! task processes them serially, which gives mutual exclusion for free.

pub mod actor;
pub mod gpio;
pub mod mode;
pub mod scheduler;

pub use actor::{SmartGridError, SmartGridHandle};
pub use mode::SmartGridMode;
