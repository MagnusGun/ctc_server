//! Heat pump status polling loop
//!
//! Polls the heat pump status and outdoor temperature registers at a configurable
//! interval to track compressor cycles and correlate with temperature.

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::interval;
use tracing::{debug, error, info, trace};

use crate::heatpump::stats::HeatPumpStats;
use crate::modbus::bms_parameters::{CTC_OUTDOOR_TEMP, HEATPUMP_STATUS};
use crate::modbus::{ModbusResponse, ModbusSender, ParameterOperation};

/// Run the heat pump status polling loop
///
/// Polls `HEATPUMP_STATUS` and `CTC_OUTDOOR_TEMP` at the specified interval
/// and updates the statistics tracker with state changes.
///
/// # Arguments
/// * `modbus_tx` - Channel to send Modbus requests
/// * `stats` - Heat pump statistics tracker
/// * `poll_interval_secs` - Polling interval in seconds
/// * `request_timeout_secs` - Timeout for each Modbus request
pub async fn run_poll_loop(
    modbus_tx: ModbusSender,
    stats: HeatPumpStats,
    poll_interval_secs: u64,
    request_timeout_secs: u64,
) {
    let mut ticker = interval(Duration::from_secs(poll_interval_secs));
    let request_timeout = Duration::from_secs(request_timeout_secs);

    info!(
        "Heat pump status polling started (interval: {}s)",
        poll_interval_secs
    );

    loop {
        ticker.tick().await;

        // Read heat pump status
        let hp_status = read_register(&modbus_tx, &HEATPUMP_STATUS, request_timeout).await;

        // Read outdoor temperature
        let outdoor_temp = read_register(&modbus_tx, &CTC_OUTDOOR_TEMP, request_timeout).await;

        match hp_status {
            Some(hp_status_value) => {
                // Status is returned as f32 but is actually an integer code
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let status_code = hp_status_value as u16;

                trace!(
                    "Heat pump poll: status={}, outdoor_temp={:?}",
                    status_code, outdoor_temp
                );

                stats.update_state(status_code, outdoor_temp);
            }
            None => {
                debug!("Failed to read heat pump status, skipping update");
            }
        }
    }
}

/// Read a single register value with timeout
///
/// Returns `Some(value)` on success, `None` on failure
async fn read_register(
    modbus_tx: &ModbusSender,
    param: &crate::modbus::CTCModbusParameter,
    timeout: Duration,
) -> Option<f32> {
    let (response_tx, response_rx) = oneshot::channel();

    // Send read request to actor
    if modbus_tx
        .send((ParameterOperation::Read(*param), response_tx))
        .await
        .is_err()
    {
        error!(
            "Failed to send {} request to Modbus actor",
            param.description
        );
        return None;
    }

    // Wait for response with timeout
    match tokio::time::timeout(timeout, response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(value)))) => Some(value),
        Ok(Ok(Ok(ModbusResponse::RawRegisters { .. }))) => {
            error!("Unexpected RawRegisters response for {}", param.description);
            None
        }
        Ok(Ok(Err(e))) => {
            debug!("Modbus error reading {}: {}", param.description, e);
            None
        }
        Ok(Err(e)) => {
            debug!("Channel error reading {}: {}", param.description, e);
            None
        }
        Err(_) => {
            debug!("Timeout reading {}", param.description);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    // Note: Integration tests would require mocking the Modbus actor
    // These are basic unit tests for the module structure

    #[test]
    fn test_module_compiles() {
        // This test ensures the module compiles correctly
        // Actual polling tests require a running Modbus actor
    }
}
