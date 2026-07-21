// Hierophant-side verification of OpenVM proofs returned by contemplants.
//
// Trust model parity with the SP1 + Bonsai paths: hierophant re-derives every
// key and commitment from the client-uploaded ELF + app config, so a worker
// can neither substitute a different program nor return a structurally valid
// proof of the wrong execution. A failed verification strikes/drops the
// worker (handled by the caller in router.rs).
//
// Wire encodings match `cargo openvm prove` (v2) file conventions:
//   app   -> bitcode bytes of ContinuationVmProof (`.app.proof`)
//   stark -> VersionedVmStarkProof JSON (`.stark.proof`)
//   evm   -> EvmProof JSON (`.evm.proof`)
//
// Keygen caching: app proving keys are cached per app-config so repeat jobs
// against the same config skip keygen. Aggregated (stark) verification needs
// the aggregation key stack too — openvm v2's `Sdk::verify_proof` checks the
// proof against a VerificationBaseline whose verifier-key commitments come
// from the aggregation provers — so the aggregation key is assembled from
// the universal internal-recursive artifact `cargo openvm setup` writes to
// ~/.openvm/internal_recursive.pk (plus an in-process prefix keygen) when
// available, generated fully in-process otherwise (slow, RAM-heavy), and
// cached per app-config after that.

use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use network_lib::OpenVmProofMode;
use openvm_continuations::CommitBytes;
use openvm_sdk::{
    DefaultStarkEngine, SC, Sdk,
    config::{AggregationSystemParams, AppConfig},
    keygen::{AggProvingKey, AppProvingKey},
    openvm_circuit::arch::ContinuationVmProof,
    prover::verify_app_proof_with_expected_exe_commit,
    types::VersionedVmStarkProof,
};
use openvm_sdk_config::SdkVmConfig;
// openvm-sdk doesn't re-export the system-params helpers or the key types
// its ~/.openvm artifacts contain; they come from openvm-stark-sdk, pinned
// to the exact tag the openvm workspace pins (see workspace Cargo.toml).
use openvm_stark_sdk::config::{MAX_APP_LOG_STACKED_HEIGHT, app_params_with_100_bits_security};
use openvm_verify_stark_host::VmStarkProof;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// openvm-sdk's major.minor, e.g. "2.0"; surfaced by GET /openvm/version so
// clients can check compatibility of the proof formats they will download.
pub const OPENVM_VERSION: &str = openvm_sdk::OPENVM_VERSION;

type MultiStarkProvingKey =
    openvm_stark_sdk::openvm_stark_backend::keygen::types::MultiStarkProvingKey<SC>;

// App proving keys per app-config hash. AppProvingKey is Arc-backed data,
// cheap to clone; caching it lets every verification after the first skip
// app keygen by seeding a fresh Sdk through the builder.
static APP_PK_CACHE: OnceLock<Mutex<HashMap<String, AppProvingKey<SdkVmConfig>>>> =
    OnceLock::new();

// Aggregation proving keys per app-config hash (the prefix half is
// app-config-dependent in v2; the internal-recursive half is universal).
static AGG_PK_CACHE: OnceLock<Mutex<HashMap<String, AggProvingKey>>> = OnceLock::new();

// Verifies `proof_bytes` against the uploaded `elf` + `app_config_toml` for
// the given mode. Blocking: run on a blocking thread. An Err means the proof
// must be treated as invalid (worker fault), except where noted inline.
pub fn verify_openvm_proof(
    elf: &[u8],
    app_config_toml: Option<&str>,
    mode: OpenVmProofMode,
    proof_bytes: &[u8],
) -> Result<()> {
    let app_config = build_app_config(app_config_toml)?;
    let config_key = config_cache_key(app_config_toml);

    match mode {
        OpenVmProofMode::App => {
            let sdk = build_sdk(app_config, &config_key)?;
            let proof: ContinuationVmProof<SC> = bitcode::deserialize(proof_bytes)
                .map_err(|e| anyhow!("decode app proof bytes: {e}"))?;
            // Expected commitment to the program, re-derived locally from the
            // client's uploaded ELF. This is what binds the worker's proof to
            // the program the client asked about.
            let expected_exe_commit = sdk
                .app_prover(elf.to_vec())
                .map_err(|e| anyhow!("transpile + commit uploaded ELF: {e}"))?
                .app_exe_commit();
            let app_vk = sdk.app_keygen().1;
            store_app_pk(&sdk, &config_key);
            verify_app_proof_with_expected_exe_commit::<DefaultStarkEngine>(
                &app_vk,
                &proof,
                Some(expected_exe_commit),
            )
            .map_err(|e| anyhow!("app proof verification failed: {e}"))?;
            Ok(())
        }
        OpenVmProofMode::Stark => {
            let versioned: VersionedVmStarkProof = serde_json::from_slice(proof_bytes)
                .context("parse stark proof JSON (VersionedVmStarkProof)")?;
            let proof =
                VmStarkProof::try_from(versioned).map_err(|e| anyhow!("decode stark proof: {e}"))?;
            let sdk = build_sdk_with_agg(app_config, &config_key)?;
            // The baseline commits to both the uploaded program and the
            // aggregation verifier-key DAG, all re-derived locally.
            let baseline = sdk
                .prover(elf.to_vec())
                .map_err(|e| anyhow!("transpile + commit uploaded ELF: {e}"))?
                .generate_baseline();
            let agg_vk = sdk.agg_vk().as_ref().clone();
            store_app_pk(&sdk, &config_key);
            store_agg_pk(&sdk, &config_key);
            Sdk::verify_proof(agg_vk, baseline, &proof)
                .map_err(|e| anyhow!("stark proof verification failed: {e}"))?;
            Ok(())
        }
        OpenVmProofMode::Evm => {
            // The full halo2 SNARK check requires the multi-GB halo2 proving
            // key + verifier artifacts of `cargo openvm setup --evm`, which
            // this lean hierophant build does not carry. EVM proofs are made
            // for onchain verification; here we enforce what we can locally:
            // the proof parses as an EvmProof and its embedded program
            // commitment matches the commitment re-derived from the uploaded
            // ELF. That catches wrong-program and garbage responses; the
            // SNARK itself (which also binds the VM-config commitment) is
            // checked by the client's verifier contract.
            let sdk = build_sdk(app_config, &config_key)?;
            let expected_exe_commit = sdk
                .app_prover(elf.to_vec())
                .map_err(|e| anyhow!("transpile + commit uploaded ELF: {e}"))?
                .app_exe_commit();
            store_app_pk(&sdk, &config_key);
            verify_evm_commit(proof_bytes, CommitBytes::from(expected_exe_commit))
        }
    }
}

// Mirror of the contemplant's config construction (which itself mirrors the
// openvm v2 CLI) so keygen matches the proving side exactly.
fn build_app_config(app_config_toml: Option<&str>) -> Result<AppConfig<SdkVmConfig>> {
    match app_config_toml {
        Some(toml) => {
            toml::from_str(toml).map_err(|e| anyhow!("parse OpenVM app config TOML: {e}"))
        }
        None => Ok(AppConfig::riscv32(app_params_with_100_bits_security(
            MAX_APP_LOG_STACKED_HEIGHT,
        ))),
    }
}

fn config_cache_key(app_config_toml: Option<&str>) -> String {
    match app_config_toml {
        Some(toml) => format!("{:x}", Sha256::digest(toml.as_bytes())),
        None => "default-riscv32".to_string(),
    }
}

// Constructs an Sdk seeded from the app-pk cache when warm.
fn build_sdk(app_config: AppConfig<SdkVmConfig>, config_key: &str) -> Result<Sdk> {
    let cache = APP_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = cache
        .lock()
        .expect("app pk cache poisoned")
        .get(config_key)
        .cloned();
    match cached {
        Some(app_pk) => Sdk::builder()
            .app_pk(app_pk)
            .agg_params(AggregationSystemParams::default())
            .build()
            .map_err(|e| anyhow!("construct OpenVM SDK from cached app key: {e}")),
        None => Sdk::new(app_config, AggregationSystemParams::default())
            .map_err(|e| anyhow!("construct OpenVM SDK: {e}")),
    }
}

// Constructs an Sdk with aggregation state ready when it can be had cheaply
// (cache or ~/.openvm artifact); otherwise the returned SDK lazily runs the
// full aggregation keygen in-process on first use (the log calls out the
// fix).
fn build_sdk_with_agg(app_config: AppConfig<SdkVmConfig>, config_key: &str) -> Result<Sdk> {
    match ensure_agg_pk(app_config.clone(), config_key)? {
        Some(agg_pk) => {
            let app_cache = APP_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
            let cached_app = app_cache
                .lock()
                .expect("app pk cache poisoned")
                .get(config_key)
                .cloned();
            let builder = match cached_app {
                Some(app_pk) => Sdk::builder().app_pk(app_pk),
                None => Sdk::builder().app_config(app_config),
            };
            builder
                .agg_pk(agg_pk)
                .build()
                .map_err(|e| anyhow!("construct OpenVM SDK with aggregation key: {e}"))
        }
        None => build_sdk(app_config, config_key),
    }
}

// Produces the aggregation proving key without full keygen when possible:
// per-config cache first, then assembly of an in-process prefix keygen (the
// app-config-dependent half) with the universal internal-recursive key
// `cargo openvm setup` writes. Returns None when neither source is
// available; callers fall back to the SDK's lazy full keygen.
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
        warn!(
            "No pre-generated OpenVM internal-recursive key found at ~/.openvm/internal_recursive.pk; running full aggregation keygen in-process. This is slow and RAM-heavy; run `cargo openvm setup` on this hierophant host to avoid it."
        );
        return Ok(None);
    };

    info!(
        "Assembling OpenVM aggregation key: prefix keygen in-process + internal-recursive key from ~/.openvm/internal_recursive.pk"
    );
    let sdk = build_sdk(app_config, config_key)?;
    let agg_pk = AggProvingKey {
        prefix: sdk.agg_prefix_pk(),
        internal_recursive: Arc::new(internal_recursive),
    };
    store_app_pk(&sdk, config_key);
    cache
        .lock()
        .expect("agg pk cache poisoned")
        .insert(config_key.to_string(), agg_pk.clone());
    Ok(Some(agg_pk))
}

fn store_app_pk(sdk: &Sdk, config_key: &str) {
    let cache = APP_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("app pk cache poisoned");
    if !cache.contains_key(config_key) {
        cache.insert(config_key.to_string(), sdk.app_keygen().0);
    }
}

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

// Parses the worker's JSON EvmProof and checks its embedded program
// commitment against the locally derived expectation. In openvm v2,
// CommitBytes serializes as a 0x-prefixed lowercase hex string, and
// AppExecutionCommit is #[serde(flatten)]ed into the EvmProof object, so the
// field lives at the top level as `app_exe_commit`.
fn verify_evm_commit(proof_bytes: &[u8], expected_exe_commit: CommitBytes) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(proof_bytes).context("parse EVM proof JSON")?;

    if value.get("proof_data").is_none() {
        return Err(anyhow!("EVM proof JSON is missing the `proof_data` field"));
    }

    let claimed = value
        .get("app_exe_commit")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches("0x").to_ascii_lowercase())
        .ok_or_else(|| anyhow!("EVM proof JSON is missing the `app_exe_commit` field"))?;
    let expected = expected_exe_commit
        .to_string()
        .trim_start_matches("0x")
        .to_ascii_lowercase();

    if claimed != expected {
        return Err(anyhow!(
            "EVM proof app_exe_commit 0x{claimed} does not match expected commit 0x{expected} of the uploaded program"
        ));
    }
    Ok(())
}
