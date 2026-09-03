//! Progress capture for the worker's Prove path.
//!
//! OpenVM's continuation prover wraps each segment in an
//! `info_span!("prove_segment", segment = seg_idx)` (see
//! openvm vm.rs `prove_continuations`). We attach a tracing Layer that
//! counts those spans and forwards a running Prove-phase count, which the
//! request handler streams to the parent as `WorkerResponse::Progress`
//! frames. This is best-effort: if the span name ever changes upstream,
//! progress silently degrades to none and proving is entirely unaffected
//! (we never gate proving on it). We report `total = 0` (indeterminate):
//! the per-segment count is a live progress + liveness signal without a
//! percentage; a reachable total via `execute_metered` is a future refinement.

use openvm_worker_proto::ProvePhase;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tracing::span::Attributes;
use tracing::{Id, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Events the prove thread forwards to the connection thread.
pub enum WorkerEvent {
    Progress {
        phase: ProvePhase,
        done: u64,
        total: u64,
    },
    Done(anyhow::Result<Vec<u8>>),
}

/// Tracing Layer counting openvm "prove_segment" spans → Prove-phase ticks.
/// `total` is a shared cell the prove path fills from execute_metered (0
/// until/unless known); each tick reports done/total.
pub struct SegmentCountLayer {
    count: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    tx: Sender<WorkerEvent>,
}

impl SegmentCountLayer {
    pub fn new(tx: Sender<WorkerEvent>, total: Arc<AtomicU64>) -> Self {
        Self {
            count: Arc::new(AtomicU64::new(0)),
            total,
            tx,
        }
    }
}

impl<S> Layer<S> for SegmentCountLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if attrs.metadata().name() == "prove_segment" {
            let done = self.count.fetch_add(1, Ordering::Relaxed) + 1;
            // Best-effort: a full channel or dropped receiver just drops the
            // tick; proving is never blocked or failed by progress.
            let _ = self.tx.send(WorkerEvent::Progress {
                phase: ProvePhase::Prove,
                done,
                total: self.total.load(Ordering::Relaxed),
            });
        }
    }
}
