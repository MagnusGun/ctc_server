//! `SmartGrid` keepalive task
//!
//! The `SmartGrid` control mechanism requires periodic refreshing of the control
//! register (1100) every 5 minutes. If not refreshed, the control reverts to default.
//!
//! This module implements a background task that maintains the `SmartGrid` mode by
//! sending keepalive writes every 4 minutes (with 1-minute safety margin).

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::modbus::{
    ModbusSender, ParameterOperation, SMARTGRID_KEEPALIVE_INTERVAL_SECS, SmartGridMode,
};

/// Shared state for the `SmartGrid` keepalive task
#[derive(Clone)]
pub struct SmartGridKeepalive {
    /// Current `SmartGrid` mode (atomic for lock-free reads)
    current_mode: Arc<AtomicU8>,
    /// Modbus actor sender
    sender: Arc<Mutex<ModbusSender>>,
    /// Keepalive interval
    interval_secs: u64,
}

impl SmartGridKeepalive {
    /// Create a new `SmartGrid` keepalive manager
    ///
    /// # Arguments
    /// * `sender` - The Modbus actor sender channel
    /// * `initial_mode` - Initial `SmartGrid` mode (default: Normal)
    /// * `interval_secs` - Keepalive interval in seconds (default: 240 = 4 minutes)
    #[must_use]
    pub fn new(
        sender: ModbusSender,
        initial_mode: SmartGridMode,
        interval_secs: Option<u64>,
    ) -> Self {
        Self {
            current_mode: Arc::new(AtomicU8::new(initial_mode as u8)),
            sender: Arc::new(Mutex::new(sender)),
            interval_secs: interval_secs.unwrap_or(SMARTGRID_KEEPALIVE_INTERVAL_SECS),
        }
    }

    /// Update the `SmartGrid` mode
    /// This will be applied on the next keepalive cycle
    pub fn set_mode(&self, mode: SmartGridMode) {
        self.current_mode.store(mode as u8, Ordering::Relaxed);
        debug!("SmartGrid mode updated to: {}", mode);
    }

    /// Get the current `SmartGrid` mode
    #[must_use]
    pub fn get_mode(&self) -> SmartGridMode {
        let value = self.current_mode.load(Ordering::Relaxed);
        // Safe because we only store valid SmartGridMode values
        match value {
            0b0000_0000 => SmartGridMode::Normal,
            0b0100_0000 => SmartGridMode::Blocking,
            0b1000_0000 => SmartGridMode::LowPrice,
            0b1100_0000 => SmartGridMode::Overcapacity,
            _ => {
                warn!(
                    "Invalid SmartGrid mode value: 0x{:02X}, defaulting to Normal",
                    value
                );
                SmartGridMode::Normal
            }
        }
    }

    /// Run the keepalive task
    /// This should be spawned as a background task with `tokio::spawn()`
    ///
    /// The task will:
    /// 1. Wait for the configured interval (default: 4 minutes)
    /// 2. Send a write request to the Modbus actor with the current mode
    /// 3. Log success/failure
    /// 4. Repeat indefinitely
    pub async fn run(self) {
        info!(
            "SmartGrid keepalive task starting (interval: {}s)",
            self.interval_secs
        );

        let mut interval_timer = interval(Duration::from_secs(self.interval_secs));

        // Skip the first tick (immediate fire) to allow system startup
        interval_timer.tick().await;

        loop {
            // Wait for the next keepalive interval
            interval_timer.tick().await;

            let mode = self.get_mode();
            debug!("SmartGrid keepalive: refreshing mode={}", mode);

            // Create a oneshot channel for the response
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            // Send the SmartGrid write operation to the actor
            let sender = self.sender.lock().await;
            match sender
                .send((ParameterOperation::WriteSmartGrid(mode), response_tx))
                .await
            {
                Ok(()) => {
                    debug!("SmartGrid keepalive: write request sent");
                    drop(sender); // Release lock before awaiting response

                    // Wait for the response (with timeout)
                    match tokio::time::timeout(Duration::from_secs(30), response_rx).await {
                        Ok(Ok(Ok(_))) => {
                            debug!("SmartGrid keepalive: refresh successful (mode={})", mode);
                        }
                        Ok(Ok(Err(e))) => {
                            error!("SmartGrid keepalive: refresh failed - {}", e);
                        }
                        Ok(Err(e)) => {
                            error!("SmartGrid keepalive: failed to receive response - {}", e);
                        }
                        Err(_) => {
                            error!("SmartGrid keepalive: timeout waiting for response");
                        }
                    }
                }
                Err(e) => {
                    drop(sender);
                    error!("SmartGrid keepalive: failed to send to actor - {}", e);
                    // Actor channel closed, task should probably exit
                    error!("SmartGrid keepalive: actor channel closed, task terminating");
                    break;
                }
            }
        }

        error!("SmartGrid keepalive task has exited - this should not happen!");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ModbusError;
    use tokio::sync::mpsc;

    #[test]
    fn test_smartgrid_keepalive_creation() {
        let (tx, _rx) = mpsc::channel(10);
        let keepalive = SmartGridKeepalive::new(tx, SmartGridMode::Normal, Some(60));

        assert_eq!(keepalive.get_mode(), SmartGridMode::Normal);
        assert_eq!(keepalive.interval_secs, 60);
    }

    #[test]
    fn test_smartgrid_keepalive_set_mode() {
        let (tx, _rx) = mpsc::channel(10);
        let keepalive = SmartGridKeepalive::new(tx, SmartGridMode::Normal, Some(60));

        keepalive.set_mode(SmartGridMode::Blocking);
        assert_eq!(keepalive.get_mode(), SmartGridMode::Blocking);

        keepalive.set_mode(SmartGridMode::LowPrice);
        assert_eq!(keepalive.get_mode(), SmartGridMode::LowPrice);

        keepalive.set_mode(SmartGridMode::Overcapacity);
        assert_eq!(keepalive.get_mode(), SmartGridMode::Overcapacity);
    }

    #[tokio::test]
    async fn test_smartgrid_keepalive_run() {
        let (tx, mut rx) = mpsc::channel::<(
            ParameterOperation,
            tokio::sync::oneshot::Sender<Result<f32, ModbusError>>,
        )>(10);

        let keepalive = SmartGridKeepalive::new(tx, SmartGridMode::Normal, Some(1)); // 1 second for testing

        // Spawn the keepalive task
        let handle = tokio::spawn(async move {
            keepalive.run().await;
        });

        // Wait for the first keepalive message (should come after ~1 second + initial tick)
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Receive the keepalive write request
        if let Some((ParameterOperation::WriteSmartGrid(mode), response_tx)) = rx.recv().await {
            assert_eq!(mode, SmartGridMode::Normal);
            // Respond with success
            response_tx.send(Ok(0.0)).unwrap();
        } else {
            panic!("Expected WriteSmartGrid operation");
        }

        // Cancel the task
        handle.abort();
    }
}
