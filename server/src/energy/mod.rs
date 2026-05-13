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

use std::sync::OnceLock;
use std::time::Duration;

/// Shared `reqwest` client for energy-API calls (Tibber, elprisetjustnu).
/// Reuses connections and gives every caller the same 15s timeout.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client build")
    })
}
