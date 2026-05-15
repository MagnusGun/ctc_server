//! Domestic-hot-water dropdown control + Bath-scoped immersion controller.

pub mod actor;
pub mod adapters;
pub mod error;
pub mod immersion;
pub mod state;
pub mod watcher;

pub use actor::DhwHandle;
