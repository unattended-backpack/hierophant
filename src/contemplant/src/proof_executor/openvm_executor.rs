use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use network_lib::{
    ContemplantProofStatus, OpenVmProofMode, OpenVmProofRequest, ProgressUpdate,
};
use openvm_sdk::{
    Sdk, StdIn,
    config::{AggregationSystemParams, AppConfig},
    keygen::AggProvingKey,
    types::VersionedVmStarkProof,
};
use openvm_sdk_config::SdkVmConfig;
// openvm-sdk doesn't re-export the system-params helpers or the key types
// its ~/.openvm artifacts contain; they come from openvm-stark-sdk, pinned
// to the exact tag the openvm workspace pins (see workspace Cargo.toml).
use openvm_stark_sdk::config::{MAX_APP_LOG_STACKED_HEIGHT, app_params_with_100_bits_security};
use sp1_sdk::network::proto::network::ExecutionStatus;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::{sync::mpsc, time::Instant};

#[derive(Clone)]
pub struct OpenVmExecutor {
    // CPU vs CUDA. Like RISC Zero, openvm-sdk selects its proving backend at
    // *compile time*: the `cuda` cargo feature swaps the `Sdk` type alias
    // from CpuSdk to GpuSdk. The contemplant must be built with
    // `--features enable-openvm-cuda` (and launched with GPU access) for a
    // CUDA backend; the config field drives build selection + capability
    // advertising, not a runtime switch.
    pub backend: ProverBackend,
    // True when the operator has opted into EVM (halo2-wrapped) proofs. This
    // requires the KZG params + halo2 key installed by
    // `cargo openvm setup --evm` under ~/.openvm/, and a binary built with
    // `--features enable-openvm-evm` so the halo2 prover is linked in.
    pub evm_enabled: bool,
}

impl OpenVmExecutor {
    pub fn new(backend: ProverBackend, evm_enabled: bool) -> Self {
        Self {
            backend,
            evm_enabled,
        }
    }
}

// Aggregation proving keys per app-config hash. Deriving one is the heaviest
// part of stark/evm proving: the internal-recursive circuit keygen alone is
// minutes + tens of GB of RAM. openvm v2 splits the key into an app-config-
// dependent prefix (leaf + internal-for-leaf) and a universal
// internal-recursive part that `cargo openvm setup` persists to
// ~/.openvm/internal_recursive.pk, so a cold start assembles prefix keygen +
// that file when present. The assembled key is cached here so every proof
// after the first skips keygen entirely.
static AGG_PK_CACHE: OnceLock<Mutex<HashMap<String, AggProvingKey>>> = OnceLock::new();

pub(super) async fn execute(
    state: WorkerState,
    executor: OpenVmExecutor,
    proof_request: OpenVmProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
    info!(
        "Received OpenVM proof request {} (mode {}, backend {:?})",
        proof_request.request_id,
        proof_request.mode.as_str(),
        executor.backend,
    );

    let initial_status = ContemplantProofStatus::unexecuted();
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    // No cycle-accurate progress tracking is wired up for OpenVM (parity with
    // RISC Zero). Push an initial non-zero Execution update so hierophant's
    // progress watchdog knows work has started.
    state
        .proof_store_client
        .proof_progress_update(proof_request.request_id, ProgressUpdate::Execution(1))
        .await;

    let request_id = proof_request.request_id;
    let display = format!(
        "OpenVM {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );
    let evm_enabled = executor.evm_enabled;

    tokio::task::spawn(async move {
        let start_time = Instant::now();

        // openvm-sdk's keygen + prover calls are CPU-blocking, so we run them
        // on a blocking thread.
        let elf = proof_request.elf;
        let app_config_toml = proof_request.app_config_toml;
        let input = proof_request.input;
        let mode = proof_request.mode;
        // NOTE: like the RISC Zero path, `mock` is accepted but not
        // implemented for OpenVM; a real proof is produced regardless.

        let proof_res: Result<Vec<u8>> = tokio::task::spawn_blocking(move || {
            prove_blocking(elf, app_config_toml, input, mode, evm_enabled, request_id.to_string())
        })
        .await
        .map_err(|e| anyhow!("OpenVM prover join error: {e}"))
        .and_then(|inner| inner);

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        match proof_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
                    )
                    .await;
            }
            Err(e) => {
                let error_msg = format!("Error proving {display} at minute {minutes}: {e}");

                // Match the SP1 path: on prover error the contemplant exits so
                // hierophant reassigns the proof to a fresh worker.
                exit_sender
                    .send(error_msg)
                    .await
                    .context("Send exit error message to main thread")
                    .unwrap();

                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;
            }
        }
    });
}

// Builds the AppConfig both proving and (hierophant-side) verification agree
// on. Mirrors openvm v2's CLI exactly: a supplied `openvm.toml` deserializes
// straight into AppConfig (system_params defaulting when the file only has
// `[app_vm_config.*]` tables); no file means the default rv32im + io config.
pub(crate) fn build_app_config(app_config_toml: Option<&str>) -> Result<AppConfig<SdkVmConfig>> {
    match app_config_toml {
        Some(toml) => {
            toml::from_str(toml).map_err(|e| anyhow!("parse OpenVM app config TOML: {e}"))
        }
        None => Ok(AppConfig::riscv32(app_params_with_100_bits_security(
            MAX_APP_LOG_STACKED_HEIGHT,
        ))),
    }
}

fn config_cache_key(app_config_toml: Option<&String>) -> String {
    use sha2::{Digest, Sha256};
    match app_config_toml {
        Some(toml) => format!("{:x}", Sha256::digest(toml.as_bytes())),
        None => "default-riscv32".to_string(),
    }
}

// Wire encodings intentionally mirror `cargo openvm prove` file conventions so
// clients can feed downloaded bytes straight into openvm tooling:
//   app   -> bitcode bytes of ContinuationVmProof (the `.app.proof` format)
//   stark -> VersionedVmStarkProof JSON (the `.stark.proof` format)
//   evm   -> EvmProof JSON (the `.evm.proof` format)
fn prove_blocking(
    elf: Vec<u8>,
    app_config_toml: Option<String>,
    input: Vec<Vec<u8>>,
    mode: OpenVmProofMode,
    evm_enabled: bool,
    request_id: String,
) -> Result<Vec<u8>> {
    let app_config = build_app_config(app_config_toml.as_deref())?;
    let config_key = config_cache_key(app_config_toml.as_ref());

    let mut stdin = StdIn::default();
    for stream in &input {
        stdin.write_bytes(stream);
    }

    match mode {
        OpenVmProofMode::App => {
            // App keygen runs per request (cached only within this Sdk value).
            // It is the cheap keygen; parity with the SP1 executor calling
            // `setup()` per request.
            let sdk = Sdk::new(app_config, AggregationSystemParams::default())
                .map_err(|e| anyhow!("construct OpenVM SDK: {e}"))?;
            let mut prover = sdk
                .app_prover(elf)
                .map_err(|e| anyhow!("construct OpenVM app prover: {e}"))?
                .with_program_name(format!("hierophant-{request_id}"));
            let proof = prover
                .prove(stdin)
                .map_err(|e| anyhow!("OpenVM app prover error: {e}"))?;
            bitcode::serialize(&proof).map_err(|e| anyhow!("encode OpenVM app proof: {e}"))
        }
        OpenVmProofMode::Stark => {
            let sdk = build_sdk_with_agg(app_config, &config_key)?;
            let (proof, _baseline) = sdk
                .prove(elf, stdin, &[])
                .map_err(|e| anyhow!("OpenVM stark prover error: {e}"))?;
            store_agg_pk(&sdk, &config_key);
            let versioned = VersionedVmStarkProof::new(proof)
                .map_err(|e| anyhow!("encode OpenVM stark proof: {e}"))?;
            serde_json::to_vec(&versioned)
                .map_err(|e| anyhow!("serialize OpenVM stark proof JSON: {e}"))
        }
        OpenVmProofMode::Evm => {
            if !evm_enabled {
                return Err(anyhow!(
                    "EVM proofs are not enabled on this contemplant. Set `evm_enabled = true` on the openvm [[provers]] entry (or CONTEMPLANT_OPENVM_EVM=true); requires the KZG params + halo2 key installed by `cargo openvm setup --evm` and a binary built with `--features enable-openvm-evm`."
                ));
            }
            prove_evm(app_config, &config_key, elf, stdin)
        }
    }
}

// Constructs an Sdk with aggregation state ready when it can be had cheaply
// (cache or ~/.openvm artifact); otherwise the returned SDK lazily runs the
// full aggregation keygen in-process (slow, tens of GB of RAM).
fn build_sdk_with_agg(app_config: AppConfig<SdkVmConfig>, config_key: &str) -> Result<Sdk> {
    match ensure_agg_pk(app_config.clone(), config_key)? {
        Some(agg_pk) => Sdk::builder()
            .app_config(app_config)
            .agg_pk(agg_pk)
            .build()
            .map_err(|e| anyhow!("construct OpenVM SDK with aggregation key: {e}")),
        None => Sdk::new(app_config, AggregationSystemParams::default())
            .map_err(|e| anyhow!("construct OpenVM SDK: {e}")),
    }
}

// Produces the aggregation proving key without full keygen when possible:
// per-config cache first, then assembly of an in-process prefix keygen (the
// app-config-dependent half; also re-runs the cheap app keygen) with the
// universal internal-recursive key `cargo openvm setup` writes. Returns None
// when neither source is available; callers fall back to the SDK's lazy
// full keygen.
fn ensure_agg_pk(
    app_config: AppConfig<SdkVmConfig>,
    config_key: &str,
) -> Result<Option<AggProvingKey>> {
    let cache = AGG_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(agg_pk) = cache
        .lock()
        .expect("agg pk cache poisoned")
        .get(config_key)
        .cloned()
    {
        return Ok(Some(agg_pk));
    }

    let Some(internal_recursive) = try_load_internal_recursive_pk() else {
        info!(
            "No pre-generated OpenVM internal-recursive key at ~/.openvm/internal_recursive.pk; the SDK will run full aggregation keygen in-process (slow, RAM-heavy). Run `cargo openvm setup` to pre-generate it."
        );
        return Ok(None);
    };

    info!(
        "Assembling OpenVM aggregation key: prefix keygen in-process + internal-recursive key from ~/.openvm/internal_recursive.pk"
    );
    let sdk = Sdk::new(app_config, AggregationSystemParams::default())
        .map_err(|e| anyhow!("construct OpenVM SDK for prefix keygen: {e}"))?;
    let agg_pk = AggProvingKey {
        prefix: sdk.agg_prefix_pk(),
        internal_recursive: Arc::new(internal_recursive),
    };
    cache
        .lock()
        .expect("agg pk cache poisoned")
        .insert(config_key.to_string(), agg_pk.clone());
    Ok(Some(agg_pk))
}

// Snapshots the (possibly lazily generated) aggregation key of a used SDK
// into the cache so later proofs skip keygen. Cheap: the key is Arc-backed.
fn store_agg_pk(sdk: &Sdk, config_key: &str) {
    let cache = AGG_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("agg pk cache poisoned");
    if !cache.contains_key(config_key) {
        cache.insert(config_key.to_string(), sdk.agg_pk());
    }
}

// The universal internal-recursive proving key that `cargo openvm setup`
// writes; program- and app-config-independent. Missing or unreadable files
// only downgrade to in-process keygen.
fn try_load_internal_recursive_pk() -> Option<MultiStarkProvingKey> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(".openvm/internal_recursive.pk");
    if !path.exists() {
        return None;
    }
    match openvm_sdk::fs::read_object_from_file::<MultiStarkProvingKey, _>(&path) {
        Ok(pk) => {
            info!(
                "Loaded OpenVM internal-recursive proving key from {}",
                path.display()
            );
            Some(pk)
        }
        Err(e) => {
            warn!(
                "Failed to read OpenVM internal-recursive proving key at {}: {e}. Falling back to in-process keygen.",
                path.display()
            );
            None
        }
    }
}

type MultiStarkProvingKey =
    openvm_stark_sdk::openvm_stark_backend::keygen::types::MultiStarkProvingKey<openvm_sdk::SC>;

#[cfg(feature = "enable-openvm-evm")]
fn prove_evm(
    app_config: AppConfig<SdkVmConfig>,
    config_key: &str,
    elf: Vec<u8>,
    stdin: StdIn,
) -> Result<Vec<u8>> {
    // Seed whatever heavy keys the `cargo openvm setup --evm` artifacts can
    // supply; anything missing falls back to in-process keygen (the halo2
    // keygen alone needs ~70 GB of RAM).
    let agg_pk = ensure_agg_pk(app_config.clone(), config_key)?;
    let halo2_pk = try_load_halo2_pk();

    let builder = Sdk::builder().app_config(app_config);
    let builder = match agg_pk {
        Some(agg_pk) => builder.agg_pk(agg_pk),
        None => builder.agg_params(AggregationSystemParams::default()),
    };
    let builder = match halo2_pk {
        Some(halo2_pk) => builder.halo2_pk(halo2_pk),
        None => {
            info!(
                "No pre-generated OpenVM halo2 key at ~/.openvm/halo2.pk; the SDK will run halo2 keygen in-process (very slow, ~70 GB of RAM). Run `cargo openvm setup --evm` to pre-generate it."
            );
            builder
        }
    };
    let sdk = builder
        .build()
        .map_err(|e| anyhow!("construct OpenVM SDK for EVM proving: {e}"))?;

    let proof = sdk
        .prove_evm(elf, stdin, &[])
        .map_err(|e| anyhow!("OpenVM EVM prover error: {e}"))?;
    store_agg_pk(&sdk, config_key);
    serde_json::to_vec(&proof).map_err(|e| anyhow!("serialize OpenVM EVM proof: {e}"))
}

#[cfg(feature = "enable-openvm-evm")]
fn try_load_halo2_pk() -> Option<openvm_sdk::keygen::Halo2ProvingKey> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(".openvm/halo2.pk");
    if !path.exists() {
        return None;
    }
    match openvm_sdk::fs::read_halo2_pk_from_file(&path) {
        Ok(pk) => {
            info!("Loaded OpenVM halo2 proving key from {}", path.display());
            Some(pk)
        }
        Err(e) => {
            warn!(
                "Failed to read OpenVM halo2 proving key at {}: {e}. Falling back to in-process keygen.",
                path.display()
            );
            None
        }
    }
}

#[cfg(not(feature = "enable-openvm-evm"))]
fn prove_evm(
    _app_config: AppConfig<SdkVmConfig>,
    _config_key: &str,
    _elf: Vec<u8>,
    _stdin: StdIn,
) -> Result<Vec<u8>> {
    // Config validation rejects evm_enabled=true on featureless builds, so
    // reaching this arm means hierophant routed an EVM request to a worker
    // that never advertised the capability.
    Err(anyhow!(
        "This contemplant binary was built without `--features enable-openvm-evm` and cannot produce OpenVM EVM proofs."
    ))
}
