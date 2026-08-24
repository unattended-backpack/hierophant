// OpenVM fibonacci host; runs the integration-test round trip against
// Hierophant's OpenVM-shaped REST surface.
//
// There is no third-party client SDK to exercise here (OpenVM has no
// bonsai-sdk analog), so every call goes over raw `reqwest` and the wire
// format stays explicit: programs are raw guest ELFs addressed by sha256,
// inputs are bincode-serialized Vec<Vec<u8>> StdIn streams, and downloaded
// proofs use `cargo openvm prove` (v2) file conventions (bitcode bytes for
// app, VersionedVmStarkProof JSON for stark, EvmProof JSON for evm).
//
// Modes covered:
//   --proof-mode app     (default; app-level continuation STARK, cheapest)
//   --proof-mode stark   (aggregated root STARK, single compact proof)
//   --proof-mode evm     (halo2-wrapped EVM proof; requires an EVM-enabled
//                        contemplant built with --features enable-openvm-evm
//                        and provisioned via `cargo openvm setup --evm`)
//
// Client-side verification by mode:
//   app   -> full verification (openvm_sdk::prover::verify_app_proof) plus a
//            program-commitment check against the locally embedded ELF.
//   stark -> decode + user-public-values check. Re-running the cryptographic
//            verification client-side needs the aggregation verifying key,
//            whose keygen is far too heavy for a test container; hierophant
//            has already verified the proof server-side before handing it
//            out (it errors the job otherwise).
//   evm   -> JSON decode + user-public-values check, same reasoning; EVM
//            proofs are made for onchain verifier contracts.

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use fibonacci::fibonacci;
use log::{info, warn};
use openvm_sdk::{
    DefaultStarkEngine, SC, Sdk,
    config::AggregationSystemParams,
    openvm_circuit::arch::ContinuationVmProof,
    prover::verify_app_proof,
    types::VersionedVmStarkProof,
};
use openvm_stark_sdk::config::{MAX_APP_LOG_STACKED_HEIGHT, app_params_with_100_bits_security};
use openvm_stark_sdk::openvm_stark_backend::p3_field::PrimeField32;
use openvm_verify_stark_host::VmStarkProof;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{thread::sleep, time::Duration};

// Guest ELF cross-compiled and embedded at build time by build.rs via
// openvm-build (see ../build.rs).
const FIBONACCI_GUEST_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/fibonacci-guest.elf"));

#[derive(Parser)]
struct Args {
    /// Fibonacci index to compute.
    #[arg(long, default_value_t = 10)]
    n: u32,

    /// Hierophant OpenVM endpoint (root URL; `/openvm/...` paths get appended).
    #[arg(long, default_value = "http://hierophant:9010/openvm")]
    openvm_url: String,

    /// Proof mode for the job: app | stark | evm.
    #[arg(long, env = "PROOF_MODE", default_value = "app")]
    proof_mode: String,

    /// Max seconds to wait for the proof job to finish before failing the
    /// test. Modes that trigger in-process aggregation keygen on the worker
    /// (stark/evm without pre-staged ~/.openvm artifacts) need generous
    /// values.
    #[arg(long, default_value_t = 1800)]
    timeout_secs: u64,

    /// Seconds between status polls.
    #[arg(long, default_value_t = 3)]
    poll_secs: u64,
}

// Shapes of the /openvm REST bodies accepted by Hierophant. Matches what the
// server (de)serializes in src/hierophant/src/openvm/types.rs.
#[derive(Deserialize)]
struct ProgramUploadResp {
    url: String,
}

#[derive(Deserialize)]
struct InputUploadResp {
    uuid: String,
    url: String,
}

#[derive(Serialize)]
struct ProofCreateBody<'a> {
    program: &'a str,
    input: &'a str,
    proof_mode: &'a str,
}

#[derive(Deserialize)]
struct ProofCreateResp {
    uuid: String,
}

#[derive(Deserialize)]
struct ProofStatusResp {
    status: String,
    #[serde(default)]
    proof_url: Option<String>,
    #[serde(default)]
    error_msg: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let program_id = format!("{:x}", Sha256::digest(FIBONACCI_GUEST_ELF));
    info!("Guest ELF sha256 program_id: {program_id}");
    info!("Flow: proof_mode={}", args.proof_mode);

    // Generous per-request timeout: the status poll that first observes a
    // finished proof blocks while hierophant runs its (keygen-heavy,
    // first-time) server-side verification before answering.
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .context("build http client")?;
    let base = args.openvm_url.trim_end_matches('/').to_string();

    // 1. Upload the guest program (ELF) under its sha256 id.
    let upload: ProgramUploadResp = http
        .get(format!("{base}/programs/upload/{program_id}"))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| anyhow!("GET /programs/upload: {e}"))?;
    info!(
        "Uploading guest program ({} bytes) to {}",
        FIBONACCI_GUEST_ELF.len(),
        upload.url
    );
    http.put(&upload.url)
        .body(FIBONACCI_GUEST_ELF.to_vec())
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow!("PUT program: {e}"))?;

    // 2. Upload the input streams. The guest does one `read::<u32>()`, whose
    // stream must hold the openvm-serde encoding of a u32: a single 4-byte
    // little-endian word (this is exactly what StdIn::write(&n) would push).
    let streams: Vec<Vec<u8>> = vec![args.n.to_le_bytes().to_vec()];
    let input_body = bincode::serialize(&streams).context("bincode input streams")?;
    let input_upload: InputUploadResp = http
        .get(format!("{base}/inputs/upload"))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| anyhow!("GET /inputs/upload: {e}"))?;
    info!(
        "Uploading input n={} ({} bytes) as input_id {}",
        args.n,
        input_body.len(),
        input_upload.uuid
    );
    http.put(&input_upload.url)
        .body(input_body)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| anyhow!("PUT input: {e}"))?;

    // 3. Create the proof job.
    let create: ProofCreateResp = http
        .post(format!("{base}/proofs/create"))
        .json(&ProofCreateBody {
            program: &program_id,
            input: &input_upload.uuid,
            proof_mode: &args.proof_mode,
        })
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| anyhow!("POST /proofs/create: {e}"))?;
    info!("Proof job uuid: {}", create.uuid);

    // 4. Poll until SUCCEEDED, then download the proof bytes.
    let proof_bytes = poll_job(&http, &base, &create.uuid, &args)?;
    info!("Received proof ({} bytes)", proof_bytes.len());

    // 5. Verify client-side (see module comment for what each mode checks)
    // and extract the user public values.
    let public_values = match args.proof_mode.as_str() {
        "app" => verify_app(&proof_bytes)?,
        "stark" => decode_stark_public_values(&proof_bytes)?,
        "evm" => decode_evm_public_values(&proof_bytes)?,
        other => bail!("unknown proof mode {other}"),
    };

    // 6. Public values are the guest's 32-byte reveal space:
    // [n, a, b, 0, ...] as little-endian u32s.
    if public_values.len() < 12 {
        bail!(
            "expected at least 12 bytes of user public values, got {}",
            public_values.len()
        );
    }
    let read_u32 =
        |i: usize| u32::from_le_bytes(public_values[i * 4..i * 4 + 4].try_into().unwrap());
    let (j_n, j_a, j_b) = (read_u32(0), read_u32(1), read_u32(2));
    info!("Public values: n={j_n}, a={j_a}, b={j_b}");

    let (exp_a, exp_b) = fibonacci(args.n);
    if j_n != args.n || j_a != exp_a || j_b != exp_b {
        bail!(
            "public values mismatch: expected (n={}, a={}, b={}), got (n={j_n}, a={j_a}, b={j_b})",
            args.n,
            exp_a,
            exp_b
        );
    }

    println!(
        "OK openvm fibonacci(n={}) = {} verified end-to-end [mode={}]",
        args.n, j_b, args.proof_mode
    );
    Ok(())
}

fn poll_job(
    http: &reqwest::blocking::Client,
    base: &str,
    job_id: &str,
    args: &Args,
) -> Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(args.timeout_secs);
    loop {
        if std::time::Instant::now() >= deadline {
            bail!(
                "timed out after {}s waiting for proof job {job_id}",
                args.timeout_secs
            );
        }

        let res: ProofStatusResp = http
            .get(format!("{base}/proofs/status/{job_id}"))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|e| anyhow!("GET /proofs/status: {e}"))?;
        match res.status.as_str() {
            "RUNNING" => {
                info!("proof job running (state={:?})", res.state);
                sleep(Duration::from_secs(args.poll_secs));
            }
            "SUCCEEDED" => {
                let url = res
                    .proof_url
                    .ok_or_else(|| anyhow!("SUCCEEDED status had no proof_url"))?;
                info!("proof job succeeded; downloading proof from {url}");
                let bytes = http
                    .get(&url)
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| anyhow!("GET {url}: {e}"))?
                    .bytes()
                    .map_err(|e| anyhow!("read proof bytes: {e}"))?;
                return Ok(bytes.to_vec());
            }
            "FAILED" => {
                bail!(
                    "proof job failed: {}",
                    res.error_msg.unwrap_or_else(|| "<no error msg>".into())
                );
            }
            other => {
                warn!("unexpected proof job status {other:?}; will retry");
                sleep(Duration::from_secs(args.poll_secs));
            }
        }
    }
}

// Full cryptographic verification of an app proof plus the program-identity
// check, mirroring what hierophant does server-side. Returns the verified
// user public values.
fn verify_app(proof_bytes: &[u8]) -> Result<Vec<u8>> {
    let proof: ContinuationVmProof<SC> =
        bitcode::deserialize(proof_bytes).map_err(|e| anyhow!("decode app proof: {e}"))?;

    // The guest is plain rv32im+io (no openvm.toml), so the default riscv32
    // config matches both the transpilation and the keygen the network used.
    let sdk = Sdk::riscv32(
        app_params_with_100_bits_security(MAX_APP_LOG_STACKED_HEIGHT),
        AggregationSystemParams::default(),
    );
    info!("Running app keygen for client-side verification (this takes a moment)...");
    let (_app_pk, app_vk) = sdk.app_keygen();
    let exe_commit = verify_app_proof::<DefaultStarkEngine>(&app_vk, &proof)
        .map_err(|e| anyhow!("verify_app_proof: {e}"))?;

    let expected = sdk
        .app_prover(FIBONACCI_GUEST_ELF.to_vec())
        .map_err(|e| anyhow!("commit local guest ELF: {e}"))?
        .app_exe_commit();
    if exe_commit != expected {
        bail!("proof exe commit does not match local guest ELF commit");
    }
    info!("App proof verified against local guest ELF commitment.");

    // Verified above (the public values Merkle proof is checked as part of
    // verify_app_proof); the reveal space cells are bytes.
    proof
        .user_public_values
        .public_values
        .iter()
        .map(|f| {
            u8::try_from(f.as_canonical_u32())
                .map_err(|_| anyhow!("public value field element out of byte range"))
        })
        .collect()
}

fn decode_stark_public_values(proof_bytes: &[u8]) -> Result<Vec<u8>> {
    let versioned: VersionedVmStarkProof =
        serde_json::from_slice(proof_bytes).context("parse stark proof JSON")?;
    let proof =
        VmStarkProof::try_from(versioned).map_err(|e| anyhow!("decode stark proof: {e}"))?;
    info!("Stark proof decoded; trusting hierophant's server-side verification (see module comment).");
    proof
        .user_pvs_proof
        .public_values
        .iter()
        .map(|f| {
            u8::try_from(f.as_canonical_u32())
                .map_err(|_| anyhow!("public value field element out of byte range"))
        })
        .collect()
}

fn decode_evm_public_values(proof_bytes: &[u8]) -> Result<Vec<u8>> {
    let value: serde_json::Value =
        serde_json::from_slice(proof_bytes).context("parse EVM proof JSON")?;
    let hex_str = value
        .get("user_public_values")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("EVM proof JSON missing user_public_values"))?;
    info!("EVM proof decoded; verify onchain with the OpenVmHalo2Verifier contract for full assurance.");
    hex::decode(hex_str.trim_start_matches("0x")).context("decode user_public_values hex")
}
