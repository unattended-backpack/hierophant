//! Wire types, framing, and the parent-side client for the
//! `openvm-worker` subprocess.
//!
//! The worker quarantines openvm-sdk (and its exact-pinned upstream
//! `p3-*` crates) into a standalone binary so the hierophant and
//! contemplant can link sp1-sdk 6.5+ (whose slop stack pins the
//! Succinct `p3-*` forks at the same 0.4 minor, which cargo cannot
//! unify in one graph). The parent spawns the worker with a unix socket
//! path; every request/response is one length-prefixed bincode frame on
//! a connection to that socket. Both existing callers already run their
//! OpenVM work on blocking threads, so this client is deliberately
//! synchronous std I/O.

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Env var naming the worker binary; falls back to [`DEFAULT_WORKER_BIN`]
/// resolved through PATH.
pub const WORKER_PATH_ENV: &str = "OPENVM_WORKER_PATH";
pub const DEFAULT_WORKER_BIN: &str = "openvm-worker";

/// Upper bound on a single frame. Proof payloads are at most tens of MB;
/// anything near this cap is a protocol error, not a big proof.
pub const MAX_FRAME_BYTES: u32 = 1 << 30;

/// How long [`WorkerClient::call_blocking`] tolerates connection refusals
/// while a freshly spawned worker binds its socket.
const STARTUP_GRACE: Duration = Duration::from_secs(60);

/// Mirrors network-lib's OpenVmProofMode without depending on network-lib
/// (which links sp1-sdk). Conversions live with the callers.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ProofMode {
    App,
    Stark,
    Evm,
}

/// Mirrors network-lib's ProvePhase (proto cannot depend on network-lib,
/// which links sp1-sdk). The caller maps this back onto the wire enum.
#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum ProvePhase {
    Execute,
    Prove,
    Aggregate,
    Wrap,
}

impl ProofMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::App => "APP",
            Self::Stark => "STARK",
            Self::Evm => "EVM",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum WorkerRequest {
    /// Produce a proof; the response is the mode's wire encoding
    /// (app -> bitcode ContinuationVmProof, stark -> VersionedVmStarkProof
    /// JSON, evm -> EvmProof JSON), exactly as before the split.
    Prove {
        request_id: String,
        mode: ProofMode,
        elf: Vec<u8>,
        app_config_toml: Option<String>,
        input: Vec<Vec<u8>>,
        // When true, run execute_metered before proving to learn the exact
        // segment total, so Progress frames carry a percentage. Cheap
        // relative to proving.
        with_total: bool,
    },
    /// Verify a proof against the uploaded ELF + app config; VerifyOk means
    /// the proof is valid for the re-derived program commitment.
    Verify {
        mode: ProofMode,
        elf: Vec<u8>,
        app_config_toml: Option<String>,
        proof_bytes: Vec<u8>,
    },
    /// openvm-sdk's OPENVM_VERSION (major.minor), for /openvm/version.
    Version,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum WorkerResponse {
    Proof(Vec<u8>),
    VerifyOk,
    Version(String),
    /// Request-level failure (guest fault, invalid proof, bad config).
    /// Transport-level failures surface as io::Error from the call path
    /// and mean the worker itself is unhealthy; a WorkerResponse::Err
    /// means the worker is alive and rejected THIS request.
    Err(String),
    /// A non-terminal progress tick emitted during a Prove request. Zero
    /// or more of these arrive on the connection before the terminal
    /// Proof/Err frame. `total == 0` means indeterminate (live count, no
    /// reachable total).
    Progress { phase: ProvePhase, done: u64, total: u64 },
}

impl WorkerResponse {
    /// A terminal frame ends the request; a Progress frame does not.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, WorkerResponse::Progress { .. })
    }
}

fn to_io(e: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = bincode::serialize(msg).map_err(to_io)?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    bincode::deserialize(&buf).map_err(to_io)
}

/// One request then read frames until a terminal one, forwarding every
/// Progress frame to `on_progress`. Blocking; run on a blocking thread. An
/// Err here is a TRANSPORT failure (worker dead, socket gone) as opposed to
/// WorkerResponse::Err, which is the request's own failure. A verify/version
/// request simply gets its one terminal frame with no Progress in between.
pub fn call<F: FnMut(ProvePhase, u64, u64)>(
    socket: &Path,
    req: &WorkerRequest,
    mut on_progress: F,
) -> io::Result<WorkerResponse> {
    let mut stream = UnixStream::connect(socket)?;
    write_frame(&mut stream, req)?;
    loop {
        let frame: WorkerResponse = read_frame(&mut stream)?;
        match frame {
            WorkerResponse::Progress { phase, done, total } => on_progress(phase, done, total),
            terminal => return Ok(terminal),
        }
    }
}

/// Parent-side handle owning the worker child process. Spawns lazily,
/// respawns after a death on the next call, and inherits stdio so worker
/// logs land in the parent's stream. Death POLICY (how many consecutive
/// transport failures demote a capability) belongs to the caller; this
/// type only supplies the mechanics.
pub struct WorkerClient {
    bin: PathBuf,
    socket: PathBuf,
    extra_args: Vec<String>,
    child: Mutex<Option<Child>>,
}

impl WorkerClient {
    /// `extra_args` are appended after `--socket <path>` (e.g.
    /// `["--backend", "cuda", "--evm"]`). The binary comes from
    /// [`WORKER_PATH_ENV`] or PATH.
    pub fn new(socket: PathBuf, extra_args: Vec<String>) -> Self {
        let bin = std::env::var_os(WORKER_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_WORKER_BIN));
        Self {
            bin,
            socket,
            extra_args,
            child: Mutex::new(None),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Spawn the worker if it is not currently running (never spawned,
    /// exited, or killed via [`Self::mark_dead`]).
    pub fn ensure_running(&self) -> io::Result<()> {
        let mut slot = self.child.lock().expect("worker child lock poisoned");
        let needs_spawn = match slot.as_mut() {
            None => true,
            Some(child) => child.try_wait()?.is_some(),
        };
        if needs_spawn {
            let _ = std::fs::remove_file(&self.socket);
            let child = Command::new(&self.bin)
                .arg("--socket")
                .arg(&self.socket)
                .args(&self.extra_args)
                .spawn()
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("spawn {} ({e}); set {WORKER_PATH_ENV} or install the worker on PATH", self.bin.display()),
                    )
                })?;
            *slot = Some(child);
        }
        Ok(())
    }

    /// Kill and forget the current child so the next call respawns it.
    pub fn mark_dead(&self) {
        let mut slot = self.child.lock().expect("worker child lock poisoned");
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Ensure the worker runs, then perform one round trip. Connection
    /// refusals during the startup grace window are retried (a fresh
    /// child is still binding); an exit during startup surfaces as an
    /// error carrying the exit status. Any transport failure after the
    /// request went out kills the child (next call respawns) and returns
    /// the error to the caller for its own death accounting.
    pub fn call_blocking<F: FnMut(ProvePhase, u64, u64)>(
        &self,
        req: &WorkerRequest,
        mut on_progress: F,
    ) -> io::Result<WorkerResponse> {
        self.ensure_running()?;
        let deadline = Instant::now() + STARTUP_GRACE;
        loop {
            // Retries only happen on connect failure, before any proving
            // begins, so re-sending the request emits no duplicate progress.
            match call(&self.socket, req, &mut on_progress) {
                Ok(resp) => return Ok(resp),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) && Instant::now() < deadline =>
                {
                    // Still binding, or died at startup? An exited child
                    // will never bind; report its status instead of
                    // spinning out the grace window.
                    {
                        let mut slot =
                            self.child.lock().expect("worker child lock poisoned");
                        if let Some(child) = slot.as_mut() {
                            if let Some(status) = child.try_wait()? {
                                slot.take();
                                return Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    format!("openvm-worker exited at startup: {status}"),
                                ));
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => {
                    self.mark_dead();
                    return Err(e);
                }
            }
        }
    }
}
