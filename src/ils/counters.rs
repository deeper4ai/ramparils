//! Per-run counters behind the end-of-run `ils: summary` line.
//!
//! Globals rather than a `&mut Stats` threaded through six signatures: they
//! are bumped on the hot path from three different functions, and one
//! process runs one tuning run. [`super::run`] resets them on entry.

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub static EVALS: AtomicUsize = AtomicUsize::new(0);
pub static CAPPED: AtomicUsize = AtomicUsize::new(0);
/// Distinct from `CAPPED` (Risks, FUTURETELL.md): a checkpoint rejection
/// is a statistical heuristic, not a proof, so a run summary should be
/// able to tell the two apart rather than reading one combined number.
pub static FUTURE_TELLING_REJECTED: AtomicUsize = AtomicUsize::new(0);

pub fn reset() {
    EVALS.store(0, Relaxed);
    CAPPED.store(0, Relaxed);
    FUTURE_TELLING_REJECTED.store(0, Relaxed);
}
/// One configuration evaluated to a verdict — completed or capped. A
/// neighbour cancelled mid-flight because another one improved first
/// reached no verdict and is deliberately not counted.
pub fn eval(capped: bool) {
    EVALS.fetch_add(1, Relaxed);
    if capped {
        CAPPED.fetch_add(1, Relaxed);
    }
}
/// A checkpoint rejection, counted alongside (not instead of) whatever
/// `eval(true)` call already covers it as a generic incomplete verdict.
pub fn future_telling_rejected() {
    FUTURE_TELLING_REJECTED.fetch_add(1, Relaxed);
}
pub fn get() -> (usize, usize, usize) {
    (
        EVALS.load(Relaxed),
        CAPPED.load(Relaxed),
        FUTURE_TELLING_REJECTED.load(Relaxed),
    )
}
