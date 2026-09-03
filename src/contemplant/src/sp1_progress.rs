//! Best-effort SP1 progress via the gpu-server's inherited stderr.
//!
//! SP1's CUDA proving is opaque in-process: `sp1-cuda` sends one bulk
//! `ProveWithMode` request to `sp1-gpu-server` and blocks on a single
//! response (there is no per-shard client loop to hook). But `sp1-cuda`
//! spawns the server with `Stdio::inherit()` on stdout AND stderr, so the
//! server writes to THIS process's stderr. We redirect our own stderr
//! (fd 2) through a pipe once at startup, and a reader thread tees every
//! line back to the real stderr while scanning for the server's progress
//! markers. The server IS the sp1 prover crates compiled as a binary, so
//! its lines are their `tracing` strings; these are verified present in
//! the vendored gpu-server binary but are UNVERSIONED, so parsing is
//! strictly best-effort and non-fatal:
//!   - unknown lines are ignored,
//!   - if the markers ever drift, SP1 silently degrades to no progress
//!     (its prior behavior) and proving is entirely unaffected.
//! Re-check these markers on every SP1 bump (see the bump checklist).
//!
//! `install()` MUST run before the CUDA prover is built (WorkerState::new),
//! so the gpu-server child inherits the already-redirected fd 2.

use alloy_primitives::B256;
use network_lib::{ProgressUpdate, ProvePhase};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

// The SP1 proof currently being proven by this contemplant, if any. Only
// one SP1 proof runs at a time per worker, so a single slot suffices.
struct Active {
    request_id: B256,
    tx: UnboundedSender<ProgressUpdate>,
    shards: u64,
    // Exact shard total, learned from the server's core-completion line.
    // SP1 exposes no up-front shard count through any stable API (its
    // sharding is internal), so this becomes known only when core proving
    // finishes; during core the Prove ticks carry total=0 (live count).
    total: u64,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Register the SP1 proof whose progress the stderr tap should attribute
/// server lines to. Called by the SP1 executor before proving.
pub fn set_active(request_id: B256, tx: UnboundedSender<ProgressUpdate>) {
    if let Ok(mut g) = ACTIVE.lock() {
        *g = Some(Active {
            request_id,
            tx,
            shards: 0,
            total: 0,
        });
    }
}

/// Clear the active SP1 proof (idempotent, and a no-op if a newer proof
/// already replaced this one). Called by the SP1 executor on completion.
pub fn clear_active(request_id: B256) {
    if let Ok(mut g) = ACTIVE.lock() {
        if g.as_ref().map(|a| a.request_id) == Some(request_id) {
            *g = None;
        }
    }
}

// Maps one gpu-server stderr line onto the active proof's progress. Order
// matters: "Proving shard" is the hot per-shard tick; the rest are phase
// transitions. All matching is substring + best-effort.
fn parse_line(line: &str, a: &mut Active) {
    if line.contains("Proving shard") {
        a.shards += 1;
        // total is 0 until core completes; the Ord/Display treats 0 as
        // indeterminate, so this is a live count that becomes a percentage
        // only once the total is known (SP1's limitation, see the struct).
        let _ = a.tx.send(ProgressUpdate::Phase {
            phase: ProvePhase::Prove,
            done: a.shards,
            total: a.total,
        });
    } else if line.contains("core proofs completed") {
        // "... core proofs completed: N" — N is the EXACT shard total, from
        // the prover itself. Learned here (post-core), it backfills the
        // total for the aggregate/wrap phases and the final record.
        if let Some(n) = last_integer(line) {
            a.total = n;
        }
        let _ = a.tx.send(ProgressUpdate::Phase {
            phase: ProvePhase::Aggregate,
            done: 1,
            total: a.total,
        });
    } else if line.contains("prove shrink")
        || line.contains("prove plonk")
        || line.contains("prove groth")
        || line.contains("prove wrap")
    {
        let _ = a.tx.send(ProgressUpdate::Phase {
            phase: ProvePhase::Wrap,
            done: 1,
            total: a.total,
        });
    }
}

// Last non-negative integer token on a line (e.g. the N in
// "Number of core proofs completed: 80").
fn last_integer(line: &str) -> Option<u64> {
    line.split(|c: char| !c.is_ascii_digit())
        .filter(|t| !t.is_empty())
        .last()
        .and_then(|t| t.parse().ok())
}

/// Redirect fd 2 through a pipe and spawn the tee+parse reader. Safe to
/// call once; subsequent calls are no-ops. On any failure it gives up
/// quietly, leaving stderr untouched (progress just stays unavailable).
pub fn install() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: raw fd surgery done once at startup before other threads
    // depend on a particular fd-2 identity; every fd is accounted for.
    unsafe {
        let saved = libc::dup(2);
        if saved < 0 {
            return;
        }
        let mut fds = [0i32; 2];
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            libc::close(saved);
            return;
        }
        let (rd, wr) = (fds[0], fds[1]);
        if libc::dup2(wr, 2) < 0 {
            libc::close(saved);
            libc::close(rd);
            libc::close(wr);
            return;
        }
        libc::close(wr);

        let read = std::fs::File::from_raw_fd(rd);
        let mut orig = std::fs::File::from_raw_fd(saved);
        std::thread::Builder::new()
            .name("sp1-stderr-tap".into())
            .spawn(move || {
                let reader = BufReader::new(read);
                for line in reader.lines() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    // Tee first so no log line is ever lost.
                    let _ = writeln!(orig, "{line}");
                    let _ = orig.flush();
                    if let Ok(mut g) = ACTIVE.lock() {
                        if let Some(a) = g.as_mut() {
                            parse_line(&line, a);
                        }
                    }
                }
            })
            .ok();
    }
}
