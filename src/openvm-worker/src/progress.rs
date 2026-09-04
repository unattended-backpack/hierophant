//! Worker Prove-path events forwarded from the prove thread to the
//! connection thread. In the cycle-rate ETA model the worker no longer
//! streams per-segment ticks; it reports OpenVM's segment count once (the
//! size signal, from execute_metered) and then the terminal result. The
//! contemplant turns that count into a live ETA (see the contemplant's
//! rate_model), so no fragile `prove_segment`-span tapping lives here.

/// Events the prove thread forwards to the connection thread.
pub enum WorkerEvent {
    /// OpenVM segment count from the metered pass (the size signal), sent
    /// once before proving when `with_total` is set.
    Size(u64),
    /// Terminal: the encoded proof, or the request-level failure.
    Done(anyhow::Result<Vec<u8>>),
}
