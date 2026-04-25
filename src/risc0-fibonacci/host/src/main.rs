// RISC Zero fibonacci host; runs the integration-test round trip against
// Hierophant's Bonsai-shaped REST surface.
//
// Common-path calls (image upload, input upload, session status, receipt
// download) go through the real `bonsai-sdk` crate, to validate that a stock
// Bonsai SDK client works against Hierophant the same way it works against
// Risc Zero's hosted service.
//
// Calls that bonsai-sdk 1.4 doesn't plumb through verbatim (passing a
// `proof_mode` field on session/create, and the two-step STARK → Groth16
// `/snark/create` wrap flow) go over raw `reqwest` so the wire format stays
// explicit and independent of SDK version quirks.
//
// Modes covered:
//   --proof-mode composite     (default, STARK, cheapest)
//   --proof-mode succinct      (recursed STARK, one segment)
//   --proof-mode groth16       (onchain verifiable; requires Groth16-enabled worker)
//   --wrap-snark               (after STARK session succeeds, POST /snark/create
//                              to wrap into a Groth16 seal; the canonical Bonsai
//                              onchain flow; also requires Groth16-enabled worker)

use anyhow::{Context, Result, anyhow, bail};
use bonsai_sdk::blocking::Client;
use clap::Parser;
use fibonacci::fibonacci;
use log::{info, warn};
use risc0_zkvm::{Digest, Receipt};
use serde::{Deserialize, Serialize};
use std::{thread::sleep, time::Duration};

// Generated at build time by risc0-build (see ../build.rs). Defines:
//   FIBONACCI_GUEST_ELF: &[u8]
//   FIBONACCI_GUEST_ID:  [u32; 8]   // Digest as 8 little-endian u32s
// plus an entry in a `GUESTS` const slice (unused here; we have one guest).
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[derive(Parser)]
struct Args {
    /// Fibonacci index to compute.
    #[arg(long, default_value_t = 10)]
    n: u32,

    /// Hierophant Bonsai endpoint (root URL; `/bonsai/...` paths get appended).
    #[arg(long, default_value = "http://hierophant:9010/bonsai")]
    bonsai_url: String,

    /// API key for Bonsai. Hierophant currently ignores this but the SDK
    /// requires a value; an empty string is accepted.
    #[arg(long, default_value = "")]
    bonsai_key: String,

    /// STARK proof mode for the session: composite | succinct | groth16.
    /// composite and succinct are STARK variants; Groth16 is a direct onchain
    /// seal (skipping the two-step wrap flow). Requires the worker to advertise
    /// the matching capability; Groth16 in particular requires a contemplant
    /// with `groth16_enabled = true`.
    #[arg(long, env = "PROOF_MODE", default_value = "composite")]
    proof_mode: String,

    /// After the STARK session succeeds, POST /snark/create to wrap the
    /// receipt into a Groth16 seal and verify that wrapped receipt instead.
    /// This is the canonical Bonsai onchain flow. Requires a
    /// Groth16-enabled contemplant.
    #[arg(long, env = "WRAP_SNARK", default_value_t = false)]
    wrap_snark: bool,

    /// Max seconds to wait for the session (and, if --wrap-snark, the snark
    /// wrap) to finish before failing the test.
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,

    /// Seconds between status polls.
    #[arg(long, default_value_t = 3)]
    poll_secs: u64,
}

// Shape of POST /sessions/create accepted by Hierophant. Matches what the
// server deserializes in src/hierophant/src/bonsai/types.rs.
#[derive(Serialize)]
struct SessionCreateBody<'a> {
    img: &'a str,
    input: &'a str,
    assumptions: Vec<String>,
    execute_only: bool,
    proof_mode: &'a str,
}

#[derive(Deserialize)]
struct SessionCreateResp {
    uuid: String,
}

#[derive(Serialize)]
struct SnarkCreateBody<'a> {
    session_id: &'a str,
}

#[derive(Deserialize)]
struct SnarkCreateResp {
    uuid: String,
}

#[derive(Deserialize)]
struct SnarkStatusResp {
    status: String,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    error_msg: Option<String>,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let expected_image_id: Digest = FIBONACCI_GUEST_ID.into();
    let image_id_hex = format!("{}", expected_image_id);
    info!("Guest image_id: 0x{}", image_id_hex);
    info!(
        "Flow: proof_mode={}, wrap_snark={}",
        args.proof_mode, args.wrap_snark
    );

    let client = Client::from_parts(
        args.bonsai_url.clone(),
        args.bonsai_key.clone(),
        risc0_zkvm::VERSION,
    )
    .map_err(|e| anyhow!("build Bonsai client: {e}"))?;

    let http = reqwest::blocking::Client::new();

    info!("Uploading guest image to Hierophant...");
    client
        .upload_img(&image_id_hex, FIBONACCI_GUEST_ELF.to_vec())
        .map_err(|e| anyhow!("upload_img: {e}"))?;

    info!("Serializing input n={}", args.n);
    let input_u32 = risc0_zkvm::serde::to_vec(&args.n).context("serialize guest input")?;
    let input_bytes: Vec<u8> = bytemuck::cast_slice(&input_u32).to_vec();
    info!("Uploading input ({} bytes)...", input_bytes.len());
    let input_id = client
        .upload_input(input_bytes)
        .map_err(|e| anyhow!("upload_input: {e}"))?;

    // Create session via raw POST so we can pass proof_mode; bonsai-sdk 1.4's
    // helper doesn't expose that field. The returned uuid is then handed to
    // the SDK's blocking::SessionId wrapper for status polling.
    info!(
        "Creating session (image_id={image_id_hex}, input_id={input_id}, mode={})",
        args.proof_mode
    );
    let sess_url = format!("{}/sessions/create", args.bonsai_url.trim_end_matches('/'));
    let sess_body = SessionCreateBody {
        img: &image_id_hex,
        input: &input_id,
        assumptions: Vec::new(),
        execute_only: false,
        proof_mode: &args.proof_mode,
    };
    let sess: SessionCreateResp = http
        .post(&sess_url)
        .json(&sess_body)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| anyhow!("POST {sess_url}: {e}"))?;
    info!("Session uuid: {}", sess.uuid);
    let session = bonsai_sdk::blocking::SessionId::new(sess.uuid.clone());

    let stark_receipt_bytes = poll_session(&client, &session, &args)?;
    info!(
        "Received STARK receipt ({} bytes)",
        stark_receipt_bytes.len()
    );

    // If --wrap-snark, kick off POST /snark/create, poll, and download the
    // wrapped (Groth16) receipt. That's what we verify at the end instead of
    // the STARK receipt.
    let receipt_bytes = if args.wrap_snark {
        info!("Requesting Groth16 wrap of session {}", sess.uuid);
        wrap_and_download(&http, &args, &sess.uuid)?
    } else {
        stark_receipt_bytes
    };

    info!(
        "Deserializing and verifying receipt ({} bytes)...",
        receipt_bytes.len()
    );
    let receipt: Receipt = bincode::deserialize(&receipt_bytes).context("deserialize receipt")?;
    receipt
        .verify(expected_image_id)
        .map_err(|e| anyhow!("receipt.verify: {e}"))?;
    info!("Receipt verified against image_id.");

    // Journal is (u32, u32, u32) = (n, fib(n-1), fib(n)). Decode and assert
    // against the same guest journal whether the receipt is a STARK or a
    // wrapped Groth16.
    let (j_n, j_a, j_b): (u32, u32, u32) = receipt
        .journal
        .decode()
        .context("decode journal as (u32, u32, u32)")?;
    info!("Journal: n={j_n}, a={j_a}, b={j_b}");

    let (exp_a, exp_b) = fibonacci(args.n);
    if j_n != args.n || j_a != exp_a || j_b != exp_b {
        bail!(
            "journal mismatch: expected (n={}, a={}, b={}), got (n={j_n}, a={j_a}, b={j_b})",
            args.n,
            exp_a,
            exp_b
        );
    }

    println!(
        "OK risc0 fibonacci(n={}) = {} verified end-to-end [mode={}, wrap={}]",
        args.n, j_b, args.proof_mode, args.wrap_snark
    );
    Ok(())
}

fn poll_session(
    client: &Client,
    session: &bonsai_sdk::blocking::SessionId,
    args: &Args,
) -> Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(args.timeout_secs);
    loop {
        if std::time::Instant::now() >= deadline {
            bail!(
                "timed out after {}s waiting for session {}",
                args.timeout_secs,
                session.uuid
            );
        }

        let res = session
            .status(client)
            .map_err(|e| anyhow!("session.status: {e}"))?;
        match res.status.as_str() {
            "RUNNING" => {
                info!(
                    "session running (state={:?}, elapsed={:?})",
                    res.state, res.elapsed_time
                );
                sleep(Duration::from_secs(args.poll_secs));
            }
            "SUCCEEDED" => {
                let url = res
                    .receipt_url
                    .ok_or_else(|| anyhow!("SUCCEEDED status had no receipt_url"))?;
                info!("session succeeded; downloading receipt from {url}");
                return client
                    .download(&url)
                    .map_err(|e| anyhow!("download receipt: {e}"));
            }
            "FAILED" => {
                bail!(
                    "session failed: {}",
                    res.error_msg.unwrap_or_else(|| "<no error msg>".into())
                );
            }
            other => {
                warn!("unexpected session status {other:?}; will retry");
                sleep(Duration::from_secs(args.poll_secs));
            }
        }
    }
}

fn wrap_and_download(
    http: &reqwest::blocking::Client,
    args: &Args,
    session_uuid: &str,
) -> Result<Vec<u8>> {
    let base = args.bonsai_url.trim_end_matches('/');
    let create: SnarkCreateResp = http
        .post(format!("{base}/snark/create"))
        .json(&SnarkCreateBody {
            session_id: session_uuid,
        })
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| anyhow!("POST /snark/create: {e}"))?;
    info!("Snark wrap uuid: {}", create.uuid);

    let deadline = std::time::Instant::now() + Duration::from_secs(args.timeout_secs);
    loop {
        if std::time::Instant::now() >= deadline {
            bail!(
                "timed out after {}s waiting for snark {}",
                args.timeout_secs,
                create.uuid
            );
        }

        let res: SnarkStatusResp = http
            .get(format!("{base}/snark/status/{}", create.uuid))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|e| anyhow!("GET /snark/status: {e}"))?;
        match res.status.as_str() {
            "RUNNING" => {
                info!("snark running");
                sleep(Duration::from_secs(args.poll_secs));
            }
            "SUCCEEDED" => {
                let url = res
                    .output
                    .ok_or_else(|| anyhow!("SUCCEEDED snark status had no output url"))?;
                info!("snark succeeded; downloading wrapped receipt from {url}");
                let bytes = http
                    .get(&url)
                    .send()
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| anyhow!("GET {url}: {e}"))?
                    .bytes()
                    .map_err(|e| anyhow!("read wrapped-receipt bytes: {e}"))?;
                return Ok(bytes.to_vec());
            }
            "FAILED" => {
                bail!(
                    "snark failed: {}",
                    res.error_msg.unwrap_or_else(|| "<no error msg>".into())
                );
            }
            other => {
                warn!("unexpected snark status {other:?}; will retry");
                sleep(Duration::from_secs(args.poll_secs));
            }
        }
    }
}

