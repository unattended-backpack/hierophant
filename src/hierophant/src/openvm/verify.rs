//! Hierophant-side verification of OpenVM proofs, via the
//! `openvm-worker` subprocess.
//!
//! The verification implementation that used to live in this file
//! (openvm-sdk keygen + Sdk::verify_proof, key caches, ~/.openvm
//! artifact loading) moved verbatim into `src/openvm-worker` at the 6.5
//! split: openvm-sdk exact-pins the upstream plonky3 crates while
//! sp1-sdk 6.2.2+ pins the Succinct forks at the same 0.4 minor, so the
//! two stacks cannot share this binary's dependency graph. The trust
//! model is unchanged: the worker re-derives every key and commitment
//! from the client-uploaded ELF + app config (see the worker's
//! verify.rs), and it runs as a local child of this hierophant, so its
//! verdict is as trusted as the in-process code was.
//!
//! A transport failure of the LOCAL worker is retried with a respawn a
//! couple of times and then surfaced as an error; note the router treats
//! a verification Err as an invalid proof (dropping the REMOTE worker),
//! so the retry loop here exists to keep a local subprocess hiccup from
//! striking an innocent contemplant.

use anyhow::{Result, anyhow};
use log::{error, warn};
use network_lib::OpenVmProofMode;
use openvm_worker_proto::{ProofMode, WorkerClient, WorkerRequest, WorkerResponse};
use std::sync::OnceLock;

/// Transport retries against the local worker before giving up (each
/// failure kills the child and the next attempt respawns it).
const VERIFY_TRANSPORT_RETRIES: usize = 3;

fn worker() -> &'static WorkerClient {
    static WORKER: OnceLock<WorkerClient> = OnceLock::new();
    WORKER.get_or_init(|| {
        let socket = std::env::temp_dir().join(format!(
            "openvm-worker-hierophant-{}.sock",
            std::process::id()
        ));
        // Verification is CPU-only and needs no EVM proving artifacts.
        let client = WorkerClient::new(socket, vec!["--backend".into(), "cpu".into()]);
        if let Err(e) = client.ensure_running() {
            error!("failed to spawn openvm-worker at startup (will retry per request): {e}");
        }
        client
    })
}

fn to_proto_mode(mode: OpenVmProofMode) -> ProofMode {
    match mode {
        OpenVmProofMode::App => ProofMode::App,
        OpenVmProofMode::Stark => ProofMode::Stark,
        OpenVmProofMode::Evm => ProofMode::Evm,
    }
}

// Verifies `proof_bytes` against the uploaded `elf` + `app_config_toml` for
// the given mode. Blocking: run on a blocking thread. An Err means the proof
// must be treated as invalid (worker fault), except the transport-exhaustion
// case called out in the module docs.
pub fn verify_openvm_proof(
    elf: &[u8],
    app_config_toml: Option<&str>,
    mode: OpenVmProofMode,
    proof_bytes: &[u8],
) -> Result<()> {
    let req = WorkerRequest::Verify {
        mode: to_proto_mode(mode),
        elf: elf.to_vec(),
        app_config_toml: app_config_toml.map(str::to_string),
        proof_bytes: proof_bytes.to_vec(),
    };
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 1..=VERIFY_TRANSPORT_RETRIES {
        // Verify emits no Progress frames; the callback is never invoked.
        match worker().call_blocking(&req, |_, _, _| {}) {
            Ok(WorkerResponse::VerifyOk) => return Ok(()),
            Ok(WorkerResponse::Err(msg)) => return Err(anyhow!("{msg}")),
            Ok(_) => return Err(anyhow!("openvm-worker returned an unexpected response kind")),
            Err(e) => {
                warn!(
                    "openvm-worker transport failure during verify \
                     (attempt {attempt}/{VERIFY_TRANSPORT_RETRIES}): {e}"
                );
                last_err = Some(e);
            }
        }
    }
    Err(anyhow!(
        "openvm-worker unavailable after {VERIFY_TRANSPORT_RETRIES} attempts: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

// openvm-sdk's major.minor, e.g. "2.0"; surfaced by GET /openvm/version so
// clients can check compatibility of the proof formats they will download.
// Queried from the worker once and cached; "unknown" when the worker is
// unavailable at first ask (callers only display this string).
pub fn openvm_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| match worker().call_blocking(&WorkerRequest::Version, |_, _, _| {}) {
            Ok(WorkerResponse::Version(v)) => v,
            Ok(_) | Err(_) => {
                warn!("could not query openvm-worker for its OPENVM_VERSION");
                "unknown".to_string()
            }
        })
        .clone()
}
