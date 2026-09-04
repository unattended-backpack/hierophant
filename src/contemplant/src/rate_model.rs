//! Per-`(VmKind, mode)` proving-throughput model, learned from this
//! contemplant's own completed proofs, used to produce a live ETA.
//!
//! Rationale (see the progress-tracking redesign): every zkVM reports a
//! cycle/segment count for a request, and proving time is well-approximated
//! by an affine function of it — `secs ~= fixed + rate * cycles` — where the
//! `fixed` term captures the ~constant SNARK-wrap/setup cost and `rate` the
//! per-cycle core-proving cost. Learning both, per `(vm, mode)`, on THIS box
//! makes the ETA adapt automatically to the GPU and to each proof system
//! without any fragile, version-coupled introspection into the provers'
//! internals (the old per-shard/segment tapping that broke across bumps).
//!
//! In-memory and per-process by design: a fresh contemplant self-calibrates
//! as it works; before it has samples for a `(vm, mode)` it honestly reports
//! that no estimate is available. Cycle *units* differ per zkVM, which is
//! fine — the model never mixes them because the key includes the `VmKind`.

use network_lib::VmKind;
use std::collections::HashMap;

/// Online accumulators for an ordinary-least-squares affine fit
/// `y = intercept + slope * x` (x = cycles, y = seconds).
#[derive(Default, Clone, Debug)]
struct Fit {
    n: u64,
    sum_x: f64,
    sum_y: f64,
    sum_xx: f64,
    sum_xy: f64,
}

impl Fit {
    fn record(&mut self, cycles: f64, secs: f64) {
        self.n += 1;
        self.sum_x += cycles;
        self.sum_y += secs;
        self.sum_xx += cycles * cycles;
        self.sum_xy += cycles * secs;
    }

    /// Estimated seconds for `cycles`, or `None` when there is no usable
    /// signal yet. With >= 2 samples that have variance in `x`, fit the full
    /// affine line (recovers `fixed` + `rate`); otherwise fall back to a
    /// proportional (through-origin) rate from the sample mean. Never returns
    /// a negative estimate.
    fn estimate(&self, cycles: f64) -> Option<f64> {
        if self.n == 0 || cycles < 0.0 {
            return None;
        }
        let denom = (self.n as f64) * self.sum_xx - self.sum_x * self.sum_x;
        let est = if self.n >= 2 && denom.abs() > 1e-6 {
            let slope = ((self.n as f64) * self.sum_xy - self.sum_x * self.sum_y) / denom;
            let intercept = (self.sum_y - slope * self.sum_x) / (self.n as f64);
            intercept + slope * cycles
        } else {
            // Not enough x-variance for an intercept: proportional guess.
            if self.sum_x <= 0.0 {
                return None;
            }
            (self.sum_y / self.sum_x) * cycles
        };
        Some(est.max(0.0))
    }
}

/// Per-`(vm, mode)` learned throughput. Cheap to clone; wrap in a
/// `Mutex`/`Arc` at the call site for shared mutation across proof tasks.
#[derive(Default, Clone, Debug)]
pub struct RateModel {
    fits: HashMap<(VmKind, String), Fit>,
}

impl RateModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completed proof: `cycles` reported by the prover, `secs` the
    /// wall-clock proving time this contemplant took for it.
    pub fn record(&mut self, vm: VmKind, mode: &str, cycles: u64, secs: f64) {
        self.fits
            .entry((vm, mode.to_string()))
            .or_default()
            .record(cycles as f64, secs);
    }

    /// `Some(estimate_secs)` if this contemplant has enough history to
    /// estimate a `(vm, mode)` proof of `cycles`; `None` otherwise.
    pub fn estimate_secs(&self, vm: VmKind, mode: &str, cycles: u64) -> Option<u64> {
        self.fits
            .get(&(vm, mode.to_string()))
            .and_then(|f| f.estimate(cycles as f64))
            .map(|s| s.round() as u64)
    }

    /// Whether any prior sample exists for this `(vm, mode)`.
    pub fn has_history(&self, vm: VmKind, mode: &str) -> bool {
        self.fits
            .get(&(vm, mode.to_string()))
            .map(|f| f.n > 0)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: u64 = 1_000_000;

    #[test]
    fn no_history_is_none() {
        let r = RateModel::new();
        assert_eq!(r.estimate_secs(VmKind::Sp1, "PLONK", 200 * M), None);
        assert!(!r.has_history(VmKind::Sp1, "PLONK"));
    }

    #[test]
    fn one_sample_is_proportional() {
        let mut r = RateModel::new();
        r.record(VmKind::Sp1, "PLONK", 200 * M, 600.0);
        assert_eq!(r.estimate_secs(VmKind::Sp1, "PLONK", 250 * M), Some(750));
    }

    #[test]
    fn two_samples_recover_fixed_plus_rate() {
        let mut r = RateModel::new();
        r.record(VmKind::Risc0, "GROTH16", 200 * M, 600.0);
        r.record(VmKind::Risc0, "GROTH16", 400 * M, 1000.0);
        assert_eq!(r.estimate_secs(VmKind::Risc0, "GROTH16", 300 * M), Some(800));
        // small proof reflects the fixed wrap cost, not a naive proportional
        assert_eq!(r.estimate_secs(VmKind::Risc0, "GROTH16", 50 * M), Some(300));
    }

    #[test]
    fn same_x_falls_back_to_proportional() {
        let mut r = RateModel::new();
        r.record(VmKind::OpenVm, "EVM", 100 * M, 400.0);
        r.record(VmKind::OpenVm, "EVM", 100 * M, 600.0);
        assert_eq!(r.estimate_secs(VmKind::OpenVm, "EVM", 100 * M), Some(500));
    }

    #[test]
    fn keys_isolate_vm_and_mode() {
        let mut r = RateModel::new();
        r.record(VmKind::Sp1, "PLONK", 200 * M, 600.0);
        assert!(r.has_history(VmKind::Sp1, "PLONK"));
        assert!(!r.has_history(VmKind::Sp1, "COMPRESSED"));
        assert!(!r.has_history(VmKind::Risc0, "PLONK"));
    }
}
