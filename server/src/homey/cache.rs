//! Last-known pump state cache shared between the [`SmartGrid`](crate::smartgrid)
//! actor, the reconciliation poller, and the `/api/v1/pump` route.
//!
//! `actual` is what the cache *most recently observed* from Homey (or what
//! the actor just successfully pushed). `stale` flips to `true` when a Homey
//! call fails so the dashboard can render a `?` instead of stale data.
//!
//! The snapshot returns `last_observed_unix_secs` (wall-clock seconds), not
//! a freshness age: the route is polled every 5 s and the dashboard de-dups
//! identical JSON, so a stable timestamp lets the chip skip re-renders while
//! the pump state is unchanged. Age is computed client-side.

use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy)]
pub struct PumpCacheSnapshot {
    pub actual: Option<bool>,
    pub last_observed_unix_secs: Option<u64>,
    pub stale: bool,
}

#[derive(Debug)]
pub struct HomeyPumpCache {
    inner: Mutex<CacheState>,
}

#[derive(Debug, Default)]
struct CacheState {
    actual: Option<bool>,
    last_updated: Option<SystemTime>,
    stale: bool,
}

impl HomeyPumpCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CacheState::default()),
        }
    }

    /// Record a successful observation (push or poll). Clears the stale flag.
    pub async fn write_fresh(&self, actual: bool) {
        let mut s = self.inner.lock().await;
        s.actual = Some(actual);
        s.last_updated = Some(SystemTime::now());
        s.stale = false;
    }

    /// Mark the cache stale without changing the recorded value. Used when a
    /// poll or push fails — `actual` may still be approximately correct so we
    /// keep it for the badge, but the dashboard renders the staleness flag.
    pub async fn mark_stale(&self) {
        let mut s = self.inner.lock().await;
        s.stale = true;
    }

    /// Read the current state.
    pub async fn read(&self) -> PumpCacheSnapshot {
        let s = self.inner.lock().await;
        PumpCacheSnapshot {
            actual: s.actual,
            last_observed_unix_secs: s
                .last_updated
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            stale: s.stale,
        }
    }
}

impl Default for HomeyPumpCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn write_fresh_records_value_and_clears_stale() {
        let cache = HomeyPumpCache::new();
        cache.mark_stale().await;
        cache.write_fresh(true).await;
        let snap = cache.read().await;
        assert_eq!(snap.actual, Some(true));
        assert!(!snap.stale);
        assert!(snap.last_observed_unix_secs.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn mark_stale_preserves_actual() {
        let cache = HomeyPumpCache::new();
        cache.write_fresh(false).await;
        cache.mark_stale().await;
        let snap = cache.read().await;
        assert_eq!(snap.actual, Some(false));
        assert!(snap.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_cache_returns_none() {
        let cache = HomeyPumpCache::new();
        let snap = cache.read().await;
        assert_eq!(snap.actual, None);
        assert_eq!(snap.last_observed_unix_secs, None);
        assert!(!snap.stale);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn last_observed_unix_secs_is_close_to_now() {
        let cache = HomeyPumpCache::new();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        cache.write_fresh(true).await;
        let snap = cache.read().await;
        let stamp = snap.last_observed_unix_secs.expect("stamp present");
        assert!(
            stamp >= before && stamp <= before + 5,
            "stamp {stamp} not within [{before}, {before}+5]"
        );
    }
}
