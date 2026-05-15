//! Hysteresis-guarded immersion-allow gate (Bath-only).

pub struct ImmersionGate {
    on_threshold: f32,
    off_threshold: f32,
    engaged: bool,
}

#[derive(Debug, PartialEq)]
pub enum ImmersionDecision {
    Engage,
    Disengage,
    NoChange,
}

impl ImmersionGate {
    #[must_use]
    pub fn new(allow_thr: f32, hyst: f32) -> Self {
        Self {
            on_threshold: allow_thr - hyst,
            off_threshold: allow_thr + hyst,
            engaged: false,
        }
    }

    /// Restore from persistence — used by crash-recovery / reload paths.
    #[must_use]
    pub fn with_engaged(allow_thr: f32, hyst: f32, engaged: bool) -> Self {
        let mut g = Self::new(allow_thr, hyst);
        g.engaged = engaged;
        g
    }

    #[cfg(test)]
    pub fn engaged(&self) -> bool {
        self.engaged
    }

    /// Decide what to do at the current tick AND apply the decision to
    /// internal state. Caller performs the side effect (write `61591`) if
    /// the result is not `NoChange`. On side-effect failure, call
    /// [`Self::revert`] so the next tick can retry.
    pub fn evaluate(&mut self, spot_sek: f32, in_cheap_band: bool) -> ImmersionDecision {
        if self.engaged {
            if !in_cheap_band || spot_sek > self.off_threshold {
                self.engaged = false;
                return ImmersionDecision::Disengage;
            }
            ImmersionDecision::NoChange
        } else if in_cheap_band && spot_sek < self.on_threshold {
            self.engaged = true;
            ImmersionDecision::Engage
        } else {
            ImmersionDecision::NoChange
        }
    }

    /// Undo the last `Engage`/`Disengage` transition. Used by the watcher
    /// when the paired Modbus write fails — keeps the gate state aligned
    /// with the heater's true state so the next tick can retry the
    /// transition instead of seeing `NoChange` against a stale view.
    pub fn revert(&mut self) {
        self.engaged = !self.engaged;
    }
}

#[cfg(test)]
mod tests {
    use super::{ImmersionDecision, ImmersionGate};

    fn gate() -> ImmersionGate {
        ImmersionGate::new(0.50, 0.05) // allow=0.50, hyst=0.05 → on<0.45, off>0.55
    }

    #[test]
    fn off_then_low_price_in_cheap_band_engages() {
        let mut g = gate();
        assert!(matches!(g.evaluate(0.40, true), ImmersionDecision::Engage));
        assert!(g.engaged());
    }

    #[test]
    fn engaged_then_price_rises_above_off_threshold_disengages() {
        let mut g = gate();
        let _ = g.evaluate(0.40, true);
        assert!(matches!(
            g.evaluate(0.60, true),
            ImmersionDecision::Disengage
        ));
        assert!(!g.engaged());
    }

    #[test]
    fn dead_zone_no_change() {
        let mut g = gate();
        let _ = g.evaluate(0.40, true);
        assert!(matches!(
            g.evaluate(0.48, true),
            ImmersionDecision::NoChange
        ));
        assert!(matches!(
            g.evaluate(0.52, true),
            ImmersionDecision::NoChange
        ));
        assert!(g.engaged());
    }

    #[test]
    fn not_in_cheap_band_never_engages_even_if_price_low() {
        let mut g = gate();
        assert!(matches!(
            g.evaluate(0.40, false),
            ImmersionDecision::NoChange
        ));
        assert!(!g.engaged());
    }

    #[test]
    fn engaged_then_band_leaves_cheap_disengages() {
        let mut g = gate();
        let _ = g.evaluate(0.40, true);
        assert!(matches!(
            g.evaluate(0.40, false),
            ImmersionDecision::Disengage
        ));
        assert!(!g.engaged());
    }

    #[test]
    fn property_writes_bounded_by_band_crossings() {
        let mut g = gate();
        let mut writes = 0;
        // Sweep: 0.6→0.4→0.5→0.6→0.4 (in cheap band throughout)
        for &p in &[0.60, 0.40, 0.50, 0.60, 0.40] {
            if !matches!(g.evaluate(p, true), ImmersionDecision::NoChange) {
                writes += 1;
            }
        }
        assert!(
            writes <= 4,
            "expected ≤ 4 writes for the sweep, got {writes}"
        );
    }

    #[test]
    fn revert_flips_engaged_back_after_failed_engage_write() {
        let mut g = gate();
        let dec = g.evaluate(0.40, true);
        assert!(matches!(dec, ImmersionDecision::Engage));
        assert!(g.engaged());
        g.revert();
        assert!(!g.engaged());
        // Next tick at same price retries Engage (would have been NoChange
        // without revert).
        assert!(matches!(g.evaluate(0.40, true), ImmersionDecision::Engage));
    }

    #[test]
    fn revert_flips_engaged_back_after_failed_disengage_write() {
        let mut g = gate();
        let _ = g.evaluate(0.40, true);
        assert!(g.engaged());
        let dec = g.evaluate(0.60, true);
        assert!(matches!(dec, ImmersionDecision::Disengage));
        assert!(!g.engaged());
        g.revert();
        assert!(g.engaged());
        // Next tick at same price retries Disengage.
        assert!(matches!(
            g.evaluate(0.60, true),
            ImmersionDecision::Disengage
        ));
    }
}
