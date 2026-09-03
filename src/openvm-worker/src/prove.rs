//! OpenVM proving, ported verbatim from the contemplant's in-process
//! `proof_executor/openvm_executor.rs` at the worker split. Behavior,
//! caches, and wire encodings are intentionally identical; only the
//! surrounding types changed (proto ProofMode instead of network-lib's
//! enum, a local ProverBackend instead of the contemplant config type).

use anyhow::{Context, Result, anyhow};
use log::{info, warn};
use openvm_worker_proto::ProofMode;
#[cfg(feature = "enable-openvm-cuda")]
use openvm_sdk::GpuSdk;
use openvm_sdk::{
    CpuSdk, StdIn,
    config::{AggregationSystemParams, AppConfig},
    keygen::{AggProvingKey, AppProvingKey},
    types::VersionedVmStarkProof,
};
use openvm_sdk_config::SdkVmConfig;
// openvm-sdk doesn't re-export the system-params helpers or the key types
// its ~/.openvm artifacts contain; they come from openvm-stark-sdk, pinned
// to the exact tag the openvm workspace pins (see this crate's Cargo.toml).
use openvm_stark_sdk::config::{MAX_APP_LOG_STACKED_HEIGHT, app_params_with_100_bits_security};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ProverBackend {
    Cpu,
    Cuda,
}

// Aggregation proving keys per app-config hash. Deriving one is the heaviest
// part of stark/evm proving: the internal-recursive circuit keygen alone is
// minutes + tens of GB of RAM. openvm v2 splits the key into an app-config-
// dependent prefix (leaf + internal-for-leaf) and a universal
// internal-recursive part that `cargo openvm setup` persists to
// ~/.openvm/internal_recursive.pk, so a cold start assembles prefix keygen +
// that file when present. The assembled key is cached here so every proof
// after the first skips keygen entirely. Because the worker is a long-lived
// daemon spawned once by its parent, this cache carries exactly the same
// warmth the in-process cache used to.
static AGG_PK_CACHE: OnceLock<Mutex<HashMap<String, AggProvingKey>>> = OnceLock::new();

// App proving keys per app-config hash. The SDK builder's dependency chain
// (`halo2_pk` -> `root_pk` -> `agg_pk` -> `app_pk`) means any seeded
// aggregation key must be accompanied by an explicit app key; app keygen is
// the cheap one, and AppProvingKey is Arc-backed so caching it per config
// makes repeat proofs skip it entirely (mirrors the verify side).
static APP_PK_CACHE: OnceLock<Mutex<HashMap<String, AppProvingKey<SdkVmConfig>>>> =
    OnceLock::new();

// Runtime backend dispatch over openvm-sdk's compile-time SDK types. With
// the `cuda` cargo feature upstream aliases `Sdk` to GpuSdk, but CpuSdk
// stays exported alongside it, so wrapping both lets the configured
// ProverBackend pick the prover at runtime. IMPORTANT CAVEAT: this honesty
// only reaches the app stage, which is engine-generic. The aggregation
// stages behind stark/evm modes are hardwired by upstream: openvm-sdk's
// AggProver (prover/agg.rs) selects InnerGpuProver by crate feature, so a
// cuda-featured binary aggregates on the GPU regardless of which SDK is
// constructed here (see the warning in prove_blocking). The proving-key
// types are engine-independent, so the caches above serve both variants; a
// worker process only ever runs the variant its --backend names.
enum AnySdk {
    Cpu(CpuSdk),
    #[cfg(feature = "enable-openvm-cuda")]
    Gpu(GpuSdk),
}

macro_rules! with_sdk {
    ($any:expr, $sdk:ident => $body:expr) => {
        match $any {
            AnySdk::Cpu($sdk) => $body,
            #[cfg(feature = "enable-openvm-cuda")]
            AnySdk::Gpu($sdk) => $body,
        }
    };
}

// The error every constructor returns for a CUDA backend in a build without
// the cuda feature. main.rs rejects that combination at startup, so
// reaching it means a routing bug rather than an operator mistake.
#[cfg(not(feature = "enable-openvm-cuda"))]
macro_rules! no_cuda_err {
    () => {
        Err(anyhow!(
            "This openvm-worker binary was built without `--features enable-openvm-cuda` and cannot serve an OpenVM CUDA backend."
        ))
    };
}

impl AnySdk {
    fn new(
        backend: ProverBackend,
        app_config: AppConfig<SdkVmConfig>,
        agg_params: AggregationSystemParams,
    ) -> Result<Self> {
        match backend {
            ProverBackend::Cpu => CpuSdk::new(app_config, agg_params)
                .map(AnySdk::Cpu)
                .map_err(|e| anyhow!("construct OpenVM CPU SDK: {e}")),
            #[cfg(feature = "enable-openvm-cuda")]
            ProverBackend::Cuda => GpuSdk::new(app_config, agg_params)
                .map(AnySdk::Gpu)
                .map_err(|e| anyhow!("construct OpenVM GPU SDK: {e}")),
            #[cfg(not(feature = "enable-openvm-cuda"))]
            ProverBackend::Cuda => no_cuda_err!(),
        }
    }

    // The builder's dependency chain requires an explicit app_pk whenever
    // agg_pk is seeded.
    fn with_keys(
        backend: ProverBackend,
        app_pk: AppProvingKey<SdkVmConfig>,
        agg_pk: AggProvingKey,
    ) -> Result<Self> {
        match backend {
            ProverBackend::Cpu => CpuSdk::builder()
                .app_pk(app_pk)
                .agg_pk(agg_pk)
                .build()
                .map(AnySdk::Cpu)
                .map_err(|e| anyhow!("construct OpenVM CPU SDK with aggregation key: {e}")),
            #[cfg(feature = "enable-openvm-cuda")]
            ProverBackend::Cuda => GpuSdk::builder()
                .app_pk(app_pk)
                .agg_pk(agg_pk)
                .build()
                .map(AnySdk::Gpu)
                .map_err(|e| anyhow!("construct OpenVM GPU SDK with aggregation key: {e}")),
            #[cfg(not(feature = "enable-openvm-cuda"))]
            ProverBackend::Cuda => no_cuda_err!(),
        }
    }

    fn app_keygen_pk(&self) -> AppProvingKey<SdkVmConfig> {
        with_sdk!(self, sdk => sdk.app_keygen().0)
    }

    // Exact segment count for a percentage denominator, via a metered
    // execution pass (no proving). Cheap relative to proving; the SDK's
    // execute_metered returns the segmentation the app prover will use.
    fn segment_count(&self, elf: Vec<u8>, stdin: StdIn) -> Result<u64> {
        with_sdk!(self, sdk => {
            let (_pv, segments) = sdk
                .execute_metered(elf, stdin)
                .map_err(|e| anyhow!("OpenVM execute_metered (for total): {e}"))?;
            Ok(segments.len() as u64)
        })
    }

    fn agg_pk(&self) -> AggProvingKey {
        with_sdk!(self, sdk => sdk.agg_pk())
    }

    fn assemble_agg_pk(&self, internal_recursive: MultiStarkProvingKey) -> AggProvingKey {
        with_sdk!(self, sdk => AggProvingKey {
            prefix: sdk.agg_prefix_pk(),
            internal_recursive: Arc::new(internal_recursive),
        })
    }

    fn prove_app(&self, elf: Vec<u8>, program_name: String, stdin: StdIn) -> Result<Vec<u8>> {
        with_sdk!(self, sdk => {
            let mut prover = sdk
                .app_prover(elf)
                .map_err(|e| anyhow!("construct OpenVM app prover: {e}"))?
                .with_program_name(program_name);
            let proof = prover
                .prove(stdin)
                .map_err(|e| anyhow!("OpenVM app prover error: {e}"))?;
            bitcode::serialize(&proof).map_err(|e| anyhow!("encode OpenVM app proof: {e}"))
        })
    }

    fn prove_stark(&self, elf: Vec<u8>, stdin: StdIn) -> Result<Vec<u8>> {
        with_sdk!(self, sdk => {
            let (proof, _baseline) = sdk
                .prove(elf, stdin, &[])
                .map_err(|e| anyhow!("OpenVM stark prover error: {e}"))?;
            let versioned = VersionedVmStarkProof::new(proof)
                .map_err(|e| anyhow!("encode OpenVM stark proof: {e}"))?;
            serde_json::to_vec(&versioned)
                .map_err(|e| anyhow!("serialize OpenVM stark proof JSON: {e}"))
        })
    }

    #[cfg(feature = "enable-openvm-evm")]
    fn prove_evm_bytes(&self, elf: Vec<u8>, stdin: StdIn) -> Result<Vec<u8>> {
        with_sdk!(self, sdk => {
            let proof = sdk
                .prove_evm(elf, stdin, &[])
                .map_err(|e| anyhow!("OpenVM EVM prover error: {e}"))?;
            serde_json::to_vec(&proof).map_err(|e| anyhow!("serialize OpenVM EVM proof: {e}"))
        })
    }

    #[cfg(feature = "enable-openvm-evm")]
    fn for_evm(
        backend: ProverBackend,
        seeded: Option<(AppProvingKey<SdkVmConfig>, AggProvingKey)>,
        app_config: AppConfig<SdkVmConfig>,
        halo2_root: Option<(
            openvm_sdk::keygen::Halo2ProvingKey,
            openvm_sdk::keygen::RootProvingKey,
        )>,
    ) -> Result<Self> {
        match backend {
            ProverBackend::Cpu => {
                let builder = match seeded {
                    Some((app_pk, agg_pk)) => CpuSdk::builder().app_pk(app_pk).agg_pk(agg_pk),
                    None => CpuSdk::builder()
                        .app_config(app_config)
                        .agg_params(AggregationSystemParams::default()),
                };
                let builder = match halo2_root {
                    Some((halo2_pk, root_pk)) => builder.root_pk(root_pk).halo2_pk(halo2_pk),
                    None => builder,
                };
                builder
                    .build()
                    .map(AnySdk::Cpu)
                    .map_err(|e| anyhow!("construct OpenVM CPU SDK for EVM proving: {e}"))
            }
            #[cfg(feature = "enable-openvm-cuda")]
            ProverBackend::Cuda => {
                let builder = match seeded {
                    Some((app_pk, agg_pk)) => GpuSdk::builder().app_pk(app_pk).agg_pk(agg_pk),
                    None => GpuSdk::builder()
                        .app_config(app_config)
                        .agg_params(AggregationSystemParams::default()),
                };
                let builder = match halo2_root {
                    Some((halo2_pk, root_pk)) => builder.root_pk(root_pk).halo2_pk(halo2_pk),
                    None => builder,
                };
                builder
                    .build()
                    .map(AnySdk::Gpu)
                    .map_err(|e| anyhow!("construct OpenVM GPU SDK for EVM proving: {e}"))
            }
            #[cfg(not(feature = "enable-openvm-cuda"))]
            ProverBackend::Cuda => no_cuda_err!(),
        }
    }
}

// Produces (and caches) the app proving key for a config, via a throwaway
// default-aggregation SDK whose only job is the app keygen.
fn ensure_app_pk(
    backend: ProverBackend,
    app_config: AppConfig<SdkVmConfig>,
    config_key: &str,
) -> Result<AppProvingKey<SdkVmConfig>> {
    let cache = APP_PK_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(app_pk) = cache
        .lock()
        .expect("app pk cache poisoned")
        .get(config_key)
        .cloned()
    {
        return Ok(app_pk);
    }
    let sdk = AnySdk::new(backend, app_config, AggregationSystemParams::default())
        .context("construct OpenVM SDK for app keygen")?;
    let app_pk = sdk.app_keygen_pk();
    cache
        .lock()
        .expect("app pk cache poisoned")
        .insert(config_key.to_string(), app_pk.clone());
    Ok(app_pk)
}

// Builds the AppConfig both proving and (hierophant-side) verification agree
// on. Mirrors openvm v2's CLI exactly: a supplied `openvm.toml` deserializes
// straight into AppConfig (system_params defaulting when the file only has
// `[app_vm_config.*]` tables); no file means the default rv32im + io config.
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
#[allow(clippy::too_many_arguments)]
pub fn prove_blocking(
    elf: Vec<u8>,
    app_config_toml: Option<String>,
    input: Vec<Vec<u8>>,
    mode: ProofMode,
    evm_enabled: bool,
    backend: ProverBackend,
    request_id: String,
    with_total: bool,
    total: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<Vec<u8>> {
    let app_config = build_app_config(app_config_toml.as_deref())?;
    let config_key = config_cache_key(app_config_toml.as_ref());

    let mut stdin = StdIn::default();
    for stream in &input {
        stdin.write_bytes(stream);
    }

    // Learn the exact segment total before proving, so the per-segment
    // ticks carry a percentage. A metered execution pass is cheap next to
    // proving; on failure we log and fall back to an indeterminate count
    // (proving is never affected).
    if with_total {
        let sdk = AnySdk::new(backend, app_config.clone(), AggregationSystemParams::default())?;
        match sdk.segment_count(elf.clone(), stdin.clone()) {
            Ok(n) => {
                info!("OpenVM segment total (metered): {n}");
                total.store(n, std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => warn!("OpenVM total computation failed, using live count: {e}"),
        }
    }

    match mode {
        ProofMode::App => {
            // App keygen runs per request (cached only within this SDK
            // value). It is the cheap keygen; parity with the SP1 executor
            // calling `setup()` per request.
            let sdk = AnySdk::new(backend, app_config, AggregationSystemParams::default())?;
            sdk.prove_app(elf, format!("hierophant-{request_id}"), stdin)
        }
        ProofMode::Stark => {
            warn_cpu_aggregation_is_gpu(backend);
            let sdk = build_sdk_with_agg(backend, app_config, &config_key)?;
            let proof_bytes = sdk.prove_stark(elf, stdin)?;
            store_agg_pk(&sdk, &config_key);
            Ok(proof_bytes)
        }
        ProofMode::Evm => {
            if !evm_enabled {
                return Err(anyhow!(
                    "EVM proofs are not enabled on this worker. Start openvm-worker with --evm (requires the KZG params + halo2 key installed by `cargo openvm setup --evm` and a binary built with `--features enable-openvm-evm`)."
                ));
            }
            warn_cpu_aggregation_is_gpu(backend);
            prove_evm(backend, app_config, &config_key, elf, stdin)
        }
    }
}

// Upstream openvm-sdk v2.0.2 pins its aggregation prover by crate feature
// (AggProver in prover/agg.rs uses InnerGpuProver whenever `cuda` is on),
// so stark/evm aggregation in a cuda-featured binary runs on the GPU even
// for a cpu-backend worker; only the app stage honors the runtime choice.
// Surface that loudly instead of silently burning GPU on a "cpu" worker.
// True CPU aggregation requires a binary built without enable-openvm-cuda.
#[cfg(feature = "enable-openvm-cuda")]
fn warn_cpu_aggregation_is_gpu(backend: ProverBackend) {
    if backend == ProverBackend::Cpu {
        warn!(
            "This openvm-worker was started with --backend cpu, but the binary was built with enable-openvm-cuda and upstream openvm-sdk selects its aggregation prover by crate feature: the aggregation stages of this stark/evm proof will run on the GPU. App proofs honor the cpu backend. For true CPU aggregation, deploy a binary built without enable-openvm-cuda."
        );
    }
}

#[cfg(not(feature = "enable-openvm-cuda"))]
fn warn_cpu_aggregation_is_gpu(_backend: ProverBackend) {}

// Constructs an SDK with aggregation state ready when it can be had cheaply
// (cache or ~/.openvm artifact); otherwise the returned SDK lazily runs the
// full aggregation keygen in-process (slow, tens of GB of RAM).
fn build_sdk_with_agg(
    backend: ProverBackend,
    app_config: AppConfig<SdkVmConfig>,
    config_key: &str,
) -> Result<AnySdk> {
    match ensure_agg_pk(backend, app_config.clone(), config_key)? {
        Some(agg_pk) => {
            let app_pk = ensure_app_pk(backend, app_config, config_key)?;
            AnySdk::with_keys(backend, app_pk, agg_pk)
        }
        None => AnySdk::new(backend, app_config, AggregationSystemParams::default()),
    }
}

// Produces the aggregation proving key without full keygen when possible:
// per-config cache first, then assembly of an in-process prefix keygen (the
// app-config-dependent half; also re-runs the cheap app keygen) with the
// universal internal-recursive key `cargo openvm setup` writes. Returns None
// when neither source is available; callers fall back to the SDK's lazy
// full keygen.
fn ensure_agg_pk(
    backend: ProverBackend,
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
    let sdk = AnySdk::new(backend, app_config, AggregationSystemParams::default())
        .context("construct OpenVM SDK for prefix keygen")?;
    let agg_pk = sdk.assemble_agg_pk(internal_recursive);
    cache
        .lock()
        .expect("agg pk cache poisoned")
        .insert(config_key.to_string(), agg_pk.clone());
    Ok(Some(agg_pk))
}

// Snapshots the (possibly lazily generated) aggregation key of a used SDK
// into the cache so later proofs skip keygen. Cheap: the key is Arc-backed.
fn store_agg_pk(sdk: &AnySdk, config_key: &str) {
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
    backend: ProverBackend,
    app_config: AppConfig<SdkVmConfig>,
    config_key: &str,
    elf: Vec<u8>,
    stdin: StdIn,
) -> Result<Vec<u8>> {
    // Seed whatever heavy keys the `cargo openvm setup --evm` artifacts can
    // supply; anything missing falls back to in-process keygen (the halo2
    // keygen alone needs ~70 GB of RAM). The SDK's builder rejects a
    // halo2_pk without a matching root_pk (the root aggregation circuit key
    // that bridges the STARK stack into the halo2 wrap), so the two are
    // loaded and applied as a pair.
    let agg_pk = ensure_agg_pk(backend, app_config.clone(), config_key)?;
    let halo2_pk = try_load_halo2_pk();
    let root_pk = try_load_root_pk();

    // Seeding agg_pk demands an explicit app_pk per the builder's dependency
    // chain; without a seedable agg key, app_config alone lets the SDK
    // keygen the whole tower lazily.
    let seeded = match agg_pk {
        Some(agg_pk) => Some((
            ensure_app_pk(backend, app_config.clone(), config_key)?,
            agg_pk,
        )),
        None => None,
    };
    let halo2_root = match (halo2_pk, root_pk) {
        (Some(halo2_pk), Some(root_pk)) => Some((halo2_pk, root_pk)),
        (Some(_), None) => {
            warn!(
                "~/.openvm/halo2.pk is present but ~/.openvm/root.pk is missing; the SDK rejects halo2_pk without root_pk, so BOTH will be keygen'd in-process (very slow, ~70 GB of RAM). Provision root.pk alongside halo2.pk (it ships in the openvm-agg-keys bundle)."
            );
            None
        }
        (None, _) => {
            info!(
                "No pre-generated OpenVM halo2 key at ~/.openvm/halo2.pk; the SDK will run halo2 keygen in-process (very slow, ~70 GB of RAM). Run `cargo openvm setup --evm` to pre-generate it."
            );
            None
        }
    };
    let sdk = AnySdk::for_evm(backend, seeded, app_config, halo2_root)?;

    let proof_bytes = sdk.prove_evm_bytes(elf, stdin)?;
    store_agg_pk(&sdk, config_key);
    Ok(proof_bytes)
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

// The root aggregation proving key `cargo openvm setup --evm` writes;
// app-config-independent, required by the SDK whenever halo2_pk is seeded.
// Missing or unreadable files downgrade to in-process keygen via the
// pair-matching in prove_evm.
#[cfg(feature = "enable-openvm-evm")]
fn try_load_root_pk() -> Option<openvm_sdk::keygen::RootProvingKey> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(".openvm/root.pk");
    if !path.exists() {
        return None;
    }
    match openvm_sdk::fs::read_object_from_file::<openvm_sdk::keygen::RootProvingKey, _>(&path) {
        Ok(pk) => {
            info!("Loaded OpenVM root proving key from {}", path.display());
            Some(pk)
        }
        Err(e) => {
            warn!(
                "Failed to read OpenVM root proving key at {}: {e}. Falling back to in-process keygen.",
                path.display()
            );
            None
        }
    }
}

#[cfg(not(feature = "enable-openvm-evm"))]
fn prove_evm(
    _backend: ProverBackend,
    _app_config: AppConfig<SdkVmConfig>,
    _config_key: &str,
    _elf: Vec<u8>,
    _stdin: StdIn,
) -> Result<Vec<u8>> {
    // main.rs rejects --evm on featureless builds at startup, so reaching
    // this arm means a routing bug rather than an operator mistake.
    Err(anyhow!(
        "This openvm-worker binary was built without `--features enable-openvm-evm` and cannot produce OpenVM EVM proofs."
    ))
}
