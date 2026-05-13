//! Spawn helper that catches panics and triggers graceful shutdown.
//!
//! Distinct from [`modbus::actor::spawn_supervised`][super::modbus::actor],
//! which respawns the Modbus actor on panic because RTU comms are critical
//! infrastructure with no fallback. The other background tasks — sensor
//! poll, hourly flush, heat-pump stats, step detector, Tibber WS, price
//! fetch — are independent enough that letting one crash silently while the
//! HTTP server keeps serving stale state is worse than exiting cleanly.

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::error;

/// Spawn a background task whose panic should bring the process down cleanly.
///
/// On panic: logs the task name and panic payload, then calls `cancel.cancel()`
/// so the rest of the system observes the shutdown signal (including the
/// graceful-shutdown flush in `main`).
pub fn spawn_with_shutdown<F>(
    name: &'static str,
    cancel: CancellationToken,
    fut: F,
) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        match AssertUnwindSafe(fut).catch_unwind().await {
            Ok(()) => {}
            Err(panic_payload) => {
                let payload = panic_payload
                    .downcast_ref::<&'static str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic payload>".to_string());
                error!("background task {name} panicked: {payload} — triggering shutdown");
                cancel.cancel();
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn ok_completion_does_not_cancel() {
        let cancel = CancellationToken::new();
        let handle = spawn_with_shutdown("noop", cancel.clone(), async {});
        handle.await.expect("task should join cleanly");
        assert!(!cancel.is_cancelled());
    }

    #[tokio::test]
    async fn panic_triggers_cancellation() {
        let cancel = CancellationToken::new();
        let handle = spawn_with_shutdown("panicker", cancel.clone(), async {
            panic!("boom");
        });
        handle.await.expect("task should join cleanly after catch_unwind");
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn pre_cancelled_token_does_not_block_task() {
        // The supervisor doesn't short-circuit on a pre-cancelled token —
        // it's the future's responsibility to observe cancellation. A simple
        // no-op task should still run to completion and the token should
        // remain cancelled afterwards.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = ran.clone();
        let handle = spawn_with_shutdown("pre-cancelled", cancel.clone(), async move {
            ran_clone.store(true, Ordering::SeqCst);
        });
        handle.await.expect("task should join cleanly");
        assert!(ran.load(Ordering::SeqCst), "task body should still execute");
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn pre_cancelled_token_propagates_to_awaiting_task() {
        // A future that awaits cancelled() against a pre-cancelled token
        // should return immediately on first poll.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let saw_cancel = Arc::new(AtomicBool::new(false));
        let sc_clone = saw_cancel.clone();
        let cancel_for_task = cancel.clone();
        let handle = spawn_with_shutdown("awaiter", cancel.clone(), async move {
            cancel_for_task.cancelled().await;
            sc_clone.store(true, Ordering::SeqCst);
        });
        handle.await.expect("task should join");
        assert!(saw_cancel.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn panic_fans_out_cancellation_to_many_cooperators() {
        // One panicker + four cooperators sharing the same token. The
        // panic must propagate cancellation to every cooperator regardless
        // of how many are subscribed.
        let cancel = CancellationToken::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(5)); // 4 cooperators + 1 panicker
        let mut handles = Vec::new();
        let counters: Vec<_> = (0..4).map(|_| Arc::new(AtomicBool::new(false))).collect();

        for (i, counter) in counters.iter().enumerate() {
            let cancel = cancel.clone();
            let barrier = barrier.clone();
            let counter = counter.clone();
            handles.push(spawn_with_shutdown(
                Box::leak(format!("cooperator-{i}").into_boxed_str()),
                cancel.clone(),
                async move {
                    barrier.wait().await;
                    cancel.cancelled().await;
                    counter.store(true, Ordering::SeqCst);
                },
            ));
        }
        let panicker = spawn_with_shutdown("panicker", cancel.clone(), {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                panic!("fan-out");
            }
        });
        panicker.await.expect("panicker joins");
        for h in handles {
            h.await.expect("cooperator joins");
        }
        for (i, counter) in counters.iter().enumerate() {
            assert!(
                counter.load(Ordering::SeqCst),
                "cooperator {i} should have seen the cancellation"
            );
        }
    }

    #[tokio::test]
    async fn panic_in_one_task_cancels_other_tasks() {
        // Use a Barrier so both tasks have demonstrably reached the same
        // point before the panicker fires — removes the 10ms sleep that
        // previously assumed the cooperator was parked.
        let cancel = CancellationToken::new();
        let cooperator_done = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let cooperator = spawn_with_shutdown("cooperator", cancel.clone(), {
            let cancel = cancel.clone();
            let done = cooperator_done.clone();
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                cancel.cancelled().await;
                done.store(true, Ordering::SeqCst);
            }
        });
        let panicker = spawn_with_shutdown("panicker", cancel.clone(), {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                panic!("kaboom");
            }
        });
        panicker.await.expect("panicker joins");
        cooperator.await.expect("cooperator joins");
        assert!(cooperator_done.load(Ordering::SeqCst));
    }
}
