//! Sensor cache poller.
//!
//! Iterates every [`Sensor`] variant on a fixed tick, reads its Modbus
//! parameter through the actor, and writes the scaled value into the
//! in-memory ring inside [`Store`]. Dashboard status routes serve from
//! that ring; this loop is the only Modbus *reader* for those routes.
//!
//! Pattern intentionally mirrors `server/src/heatpump/poller.rs`.

use std::time::{Duration, SystemTime};

use tokio::sync::oneshot;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace};

use crate::modbus::bms_parameters::{
    CTC_ACTUAL_TEMP_DHW, CTC_OUTDOOR_TEMP, CTC_RADIATOR_WATER, CTC_RETURN_TEMP, CTC_ROOM_TEMP,
    CTC_SYSTEM_STATUS, HEATPUMP_BRINE_INLET_TEMP, HEATPUMP_BRINE_OUTLET_TEMP, HEATPUMP_BRINE_PUMP,
    HEATPUMP_CHARGE_PUMP, HEATPUMP_DISCHARGE_TEMP, HEATPUMP_HIGH_PRESSURE, HEATPUMP_INLET_TEMP,
    HEATPUMP_LOW_PRESSURE, HEATPUMP_OUTLET_TEMP, HEATPUMP_STATUS, HEATPUMP_SUCTION_TEMP,
    HEATSYSTEM_FLOW_SETPOINT, HEATSYSTEM_FLOW_TEMP,
};
use crate::modbus::{CTCModbusParameter, ModbusResponse, ModbusSender, ParameterOperation};
use crate::storage::{Sensor, Store};

/// Map a Modbus register address to the [`Sensor`] that caches it, if any.
/// Used by the generic `/api/v1/ctc?addr=...` route to short-circuit reads
/// of cached sensors.
#[must_use]
pub fn sensor_for_addr(addr: u16) -> Option<Sensor> {
    SENSORS.iter().find(|(_, p)| p.id == addr).map(|(s, _)| *s)
}

/// Single source of truth for what the poller reads each tick.
/// Order is not load-bearing — add new entries anywhere.
const SENSORS: &[(Sensor, CTCModbusParameter)] = &[
    (Sensor::Room, CTC_ROOM_TEMP),
    (Sensor::Outdoor, CTC_OUTDOOR_TEMP),
    (Sensor::Flow, HEATSYSTEM_FLOW_TEMP),
    (Sensor::Return, CTC_RETURN_TEMP),
    (Sensor::FlowSp, HEATSYSTEM_FLOW_SETPOINT),
    (Sensor::HpIn, HEATPUMP_INLET_TEMP),
    (Sensor::HpOut, HEATPUMP_OUTLET_TEMP),
    (Sensor::Discharge, HEATPUMP_DISCHARGE_TEMP),
    (Sensor::Suction, HEATPUMP_SUCTION_TEMP),
    (Sensor::HighP, HEATPUMP_HIGH_PRESSURE),
    (Sensor::LowP, HEATPUMP_LOW_PRESSURE),
    (Sensor::BrineIn, HEATPUMP_BRINE_INLET_TEMP),
    (Sensor::BrineOut, HEATPUMP_BRINE_OUTLET_TEMP),
    (Sensor::ChargePump, HEATPUMP_CHARGE_PUMP),
    (Sensor::BrinePump, HEATPUMP_BRINE_PUMP),
    (Sensor::DhwUpper, CTC_ACTUAL_TEMP_DHW),
    (Sensor::Lower, CTC_RADIATOR_WATER),
    (Sensor::SystemStatus, CTC_SYSTEM_STATUS),
    (Sensor::HpStatus, HEATPUMP_STATUS),
];

/// Run the sensor cache polling loop.
///
/// Each tick reads every sensor in [`SENSORS`] and writes the scaled
/// value into `store`'s in-memory ring. Transient read failures (timeout,
/// Modbus error) are logged at trace and skipped. A closed actor channel
/// is fatal — the function returns so the supervising task observes it.
///
/// # Arguments
/// * `modbus_tx` — actor channel
/// * `store` — sensor cache
/// * `poll_interval_secs` — seconds between full sweeps
/// * `request_timeout_secs` — per-read timeout
pub async fn run_sensor_poll_loop(
    modbus_tx: ModbusSender,
    store: Store,
    poll_interval_secs: u64,
    request_timeout_secs: u64,
    cancel: CancellationToken,
) {
    let mut ticker = interval(Duration::from_secs(poll_interval_secs));
    // After a Modbus stall, default Burst behavior fires every backlogged
    // tick in rapid succession when comms recover. Delay so we resume on a
    // fresh interval boundary instead.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let request_timeout = Duration::from_secs(request_timeout_secs);

    info!(
        "Sensor cache poller started ({} sensors, interval: {}s)",
        SENSORS.len(),
        poll_interval_secs
    );

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Sensor poller: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {}
        }
        let now = SystemTime::now();

        for (sensor, param) in SENSORS {
            if cancel.is_cancelled() {
                return;
            }
            match read_register(&modbus_tx, param, request_timeout).await {
                ReadOutcome::Value(v) => match store.record_sample(*sensor, now, v) {
                    Ok(()) => trace!("polled {:?}: {}", sensor, v),
                    Err(e) => debug!("record_sample failed for {:?}: {}", sensor, e),
                },
                ReadOutcome::TransientFailure => {
                    trace!("skipped {:?}: transient read failure", sensor);
                }
                ReadOutcome::ActorClosed => {
                    error!("Modbus actor channel closed; sensor poll loop exiting");
                    return;
                }
            }
        }
    }
}

enum ReadOutcome {
    Value(f32),
    TransientFailure,
    ActorClosed,
}

async fn read_register(
    modbus_tx: &ModbusSender,
    param: &CTCModbusParameter,
    timeout: Duration,
) -> ReadOutcome {
    let (response_tx, response_rx) = oneshot::channel();

    if modbus_tx
        .send((ParameterOperation::Read(*param), response_tx))
        .await
        .is_err()
    {
        return ReadOutcome::ActorClosed;
    }

    match tokio::time::timeout(timeout, response_rx).await {
        Ok(Ok(Ok(ModbusResponse::Value(v)))) => ReadOutcome::Value(v),
        Ok(Ok(Err(e))) => {
            debug!("modbus error reading {}: {}", param.description, e);
            ReadOutcome::TransientFailure
        }
        _ => ReadOutcome::TransientFailure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn poller_records_one_sample_per_sensor() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();

        let (tx, mut rx) = mpsc::channel::<crate::modbus::actor::ModbusRequest>(64);
        // Mock actor: answer every Read with 42.0.
        let actor = tokio::spawn(async move {
            while let Some((op, resp)) = rx.recv().await {
                if let ParameterOperation::Read(_) = op {
                    let _ = resp.send(Ok(ModbusResponse::Value(42.0)));
                }
            }
        });

        let store_for_poller = store.clone();
        let poller = tokio::spawn(async move {
            run_sensor_poll_loop(tx, store_for_poller, 1, 1, CancellationToken::new()).await;
        });

        // First tick fires immediately. Give the mock + store time to drain
        // all 19 reads.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let all_present = SENSORS
                .iter()
                .all(|(s, _)| !store.series_range(*s, 0, i64::MAX).is_empty());
            if all_present {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "not all sensors recorded within 2s"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        poller.abort();
        let _ = poller.await;
        actor.abort();
        let _ = actor.await;

        for (sensor, _) in SENSORS {
            let pts = store.series_range(*sensor, 0, i64::MAX);
            assert!(!pts.is_empty(), "{sensor:?} should have a sample");
            assert!((pts[0].1 - 42.0).abs() < f32::EPSILON);
        }
    }

    #[tokio::test]
    async fn poller_exits_when_actor_channel_closes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ctc.redb")).unwrap();

        let (tx, rx) = mpsc::channel::<crate::modbus::actor::ModbusRequest>(1);
        // Drop the receiver immediately so the very first send fails.
        drop(rx);

        // Should return cleanly, not hang.
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            run_sensor_poll_loop(tx, store, 1, 1, CancellationToken::new()),
        )
        .await;
        assert!(result.is_ok(), "poller did not exit on closed actor");
    }
}
