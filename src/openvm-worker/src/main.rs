//! openvm-worker: the process boundary that keeps openvm-sdk (and its
//! upstream exact-pinned p3-* crates) out of the hierophant/contemplant
//! binaries so those can link sp1-sdk 6.5+. One worker serves one parent
//! over a unix socket: the contemplant sends Prove requests, the
//! hierophant sends Verify requests; both are blocking calls dispatched
//! on a thread per connection. Proving/verification logic is ported
//! verbatim from the pre-split in-process implementations (see prove.rs
//! and verify.rs), key caches included, so a long-lived worker keeps the
//! same warm-keygen behavior the parents had in-process.

mod progress;
mod prove;
mod verify;

use anyhow::{Result, anyhow};
use clap::Parser;
use log::{error, info, warn};
use openvm_worker_proto::{ProofMode, WorkerRequest, WorkerResponse, read_frame, write_frame};
use progress::WorkerEvent;
use prove::ProverBackend;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Parser)]
#[command(about = "OpenVM proving/verification worker (subprocess of hierophant or contemplant)")]
struct Args {
    /// Unix socket path to bind. The parent removes/creates the parent
    /// directory and connects here after spawn.
    #[arg(long)]
    socket: PathBuf,
    /// cpu or cuda. CUDA additionally requires a binary built with
    /// --features enable-openvm-cuda and a GPU at launch.
    #[arg(long, default_value = "cpu")]
    backend: String,
    /// Enable EVM (halo2-wrapped) proving. Requires the artifacts of
    /// `cargo openvm setup --evm` under ~/.openvm/ and a binary built
    /// with --features enable-openvm-evm.
    #[arg(long)]
    evm: bool,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let backend = match args.backend.to_lowercase().as_str() {
        "cpu" => ProverBackend::Cpu,
        "cuda" => ProverBackend::Cuda,
        other => return Err(anyhow!("invalid --backend '{other}' (expected cpu or cuda)")),
    };

    // Fail at startup, not per request, when the build lacks a requested
    // capability: the parent sees an immediate child exit with a clear
    // message instead of every proof failing individually.
    #[cfg(not(feature = "enable-openvm-cuda"))]
    if backend == ProverBackend::Cuda {
        return Err(anyhow!(
            "--backend cuda requires an openvm-worker built with --features enable-openvm-cuda"
        ));
    }
    #[cfg(not(feature = "enable-openvm-evm"))]
    if args.evm {
        return Err(anyhow!(
            "--evm requires an openvm-worker built with --features enable-openvm-evm"
        ));
    }

    if args.socket.exists() {
        std::fs::remove_file(&args.socket)?;
    }
    if let Some(dir) = args.socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(&args.socket)?;
    info!(
        "openvm-worker listening on {} (openvm {}, backend {:?}, evm {})",
        args.socket.display(),
        openvm_sdk::OPENVM_VERSION,
        backend,
        args.evm,
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let evm = args.evm;
                std::thread::spawn(move || handle_conn(stream, backend, evm));
            }
            Err(e) => warn!("accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_conn(mut stream: UnixStream, backend: ProverBackend, evm_enabled: bool) {
    loop {
        let req: WorkerRequest = match read_frame(&mut stream) {
            Ok(req) => req,
            // EOF (parent closed the connection) is the normal end.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return,
            Err(e) => {
                warn!("read error on worker connection: {e}");
                return;
            }
        };
        if let Err(e) = handle_request(&mut stream, req, backend, evm_enabled) {
            error!("write error on worker connection: {e}");
            return;
        }
    }
}

// Handles one request, writing its full frame sequence to `stream`: a
// Prove request streams zero or more Progress frames followed by a
// terminal Proof/Err; Verify/Version write a single terminal frame.
fn handle_request(
    stream: &mut UnixStream,
    req: WorkerRequest,
    backend: ProverBackend,
    evm_enabled: bool,
) -> std::io::Result<()> {
    match req {
        WorkerRequest::Version => write_frame(
            stream,
            &WorkerResponse::Version(openvm_sdk::OPENVM_VERSION.to_string()),
        ),
        WorkerRequest::Verify {
            mode,
            elf,
            app_config_toml,
            proof_bytes,
        } => {
            info!("verify request (mode {})", mode.as_str());
            let resp =
                match verify::verify_openvm_proof(&elf, app_config_toml.as_deref(), mode, &proof_bytes) {
                    Ok(()) => WorkerResponse::VerifyOk,
                    Err(e) => WorkerResponse::Err(format!("{e:#}")),
                };
            write_frame(stream, &resp)
        }
        WorkerRequest::Prove {
            request_id,
            mode,
            elf,
            app_config_toml,
            input,
            with_total,
        } => {
            info!("prove request {request_id} (mode {})", mode.as_str());
            // Run the CPU-blocking prove on its own thread; it reports the
            // segment count once (Size) then the terminal result over a
            // channel to this thread, which writes the corresponding frames.
            let (tx, rx) = std::sync::mpsc::channel::<WorkerEvent>();
            let jh = std::thread::spawn(move || {
                let tx_prove = tx.clone();
                let result = prove::prove_blocking(
                    elf,
                    app_config_toml,
                    input,
                    mode,
                    evm_enabled,
                    backend,
                    request_id,
                    with_total,
                    tx_prove,
                );
                let _ = tx.send(WorkerEvent::Done(result));
            });

            let mut write_res = Ok(());
            while let Ok(ev) = rx.recv() {
                match ev {
                    WorkerEvent::Size(segments) => {
                        if let Err(e) = write_frame(stream, &WorkerResponse::Size { segments }) {
                            // Parent went away; stop streaming but let the
                            // prove thread finish and self-clean.
                            write_res = Err(e);
                            break;
                        }
                    }
                    WorkerEvent::Done(result) => {
                        let resp = match result {
                            Ok(bytes) => WorkerResponse::Proof(bytes),
                            Err(e) => WorkerResponse::Err(format!("{e:#}")),
                        };
                        write_res = write_frame(stream, &resp);
                        break;
                    }
                }
            }
            let _ = jh.join();
            write_res
        }
    }
}

// Silence dead-code on ProofMode::as_str when only some arms log.
#[allow(unused)]
fn _mode_names() {
    let _ = ProofMode::App.as_str();
}
