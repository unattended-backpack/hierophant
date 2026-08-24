use crate::config::{AssessorConfig, Config, ProverBackend, VmChoice};
use crate::proof_executor::{OpenVmExecutor, Risc0Executor, Sp1Executor};
use crate::proof_store::ProofStoreClient;
use log::{error, info};
use network_lib::VmKind;
use sp1_sdk::{CpuProver, CudaProver, ProverClient};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub enum ActiveSp1Prover {
    Cuda(Arc<CudaProver>),
    Cpu(Arc<CpuProver>),
}

#[derive(Clone)]
pub struct WorkerState {
    pub sp1_executor: Option<Sp1Executor>,
    pub risc0_executor: Option<Risc0Executor>,
    pub openvm_executor: Option<OpenVmExecutor>,
    pub proof_store_client: ProofStoreClient,
    pub assessor_config: AssessorConfig,
    // just used for healthcheck.  Is set to true in api/connect_to_hierophant
    pub ready: Arc<Mutex<bool>>,
}

impl WorkerState {
    // async because sp1-sdk 6.x prover construction is async (each prover
    // spins up its local worker node); called once from main.
    pub async fn new(config: Config) -> Self {
        let mut sp1_executor: Option<Sp1Executor> = None;
        let mut risc0_executor: Option<Risc0Executor> = None;
        let mut openvm_executor: Option<OpenVmExecutor> = None;

        for prover_cfg in &config.provers {
            match prover_cfg.vm {
                VmChoice::Sp1 => {
                    let active =
                        build_sp1_active(prover_cfg.backend, &prover_cfg.moongate_endpoint).await;
                    let mock = Arc::new(ProverClient::builder().mock().build().await);
                    // Progress tracking watched the external moongate-server's
                    // log for clk/shard lines. sp1-sdk 6.x replaced moongate
                    // with a locally spawned sp1-gpu-server that offers no
                    // watchable log endpoint, so tracking is unavailable on
                    // every backend until the new server grows an equivalent.
                    let progress_tracking_available = false;
                    info!("SP1 progress tracking disabled (unavailable on sp1-sdk 6.x)");
                    sp1_executor = Some(Sp1Executor {
                        active_prover: active,
                        mock_prover: mock,
                        progress_tracking_available,
                    });
                }
                VmChoice::Risc0 => {
                    info!(
                        "RISC Zero executor enabled (backend={:?}, groth16_enabled={})",
                        prover_cfg.backend, prover_cfg.groth16_enabled
                    );
                    risc0_executor = Some(Risc0Executor::new(
                        prover_cfg.backend,
                        prover_cfg.groth16_enabled,
                    ));
                }
                VmChoice::OpenVm => {
                    info!(
                        "OpenVM executor enabled (backend={:?}, evm_enabled={})",
                        prover_cfg.backend, prover_cfg.evm_enabled
                    );
                    openvm_executor = Some(OpenVmExecutor::new(
                        prover_cfg.backend,
                        prover_cfg.evm_enabled,
                    ));
                }
            }
        }
        info!("Prover(s) built");

        let proof_store_client = ProofStoreClient::new(config.max_proofs_stored);
        let ready = Arc::new(Mutex::new(false));

        Self {
            sp1_executor,
            risc0_executor,
            openvm_executor,
            proof_store_client,
            assessor_config: config.assessor.clone(),
            ready,
        }
    }

    pub fn supported_vms(&self) -> Vec<VmKind> {
        let mut out = Vec::new();
        if self.sp1_executor.is_some() {
            out.push(VmKind::Sp1);
        }
        if self.risc0_executor.is_some() {
            out.push(VmKind::Risc0);
        }
        if self.openvm_executor.is_some() {
            out.push(VmKind::OpenVm);
        }
        out
    }

    // True iff this contemplant is configured to produce RISC Zero Groth16
    // proofs. Reported at registration so hierophant's assignment filter
    // doesn't hand Groth16 work to workers that would fail fast inside the
    // executor anyway.
    pub fn groth16_enabled(&self) -> bool {
        self.risc0_executor
            .as_ref()
            .map(|e| e.groth16_enabled)
            .unwrap_or(false)
    }

    // True iff this contemplant is configured to produce OpenVM EVM
    // (halo2-wrapped) proofs. Reported at registration so hierophant's
    // assignment filter doesn't hand EVM work to workers that would fail
    // fast inside the executor anyway.
    pub fn openvm_evm_enabled(&self) -> bool {
        self.openvm_executor
            .as_ref()
            .map(|e| e.evm_enabled)
            .unwrap_or(false)
    }
}

async fn build_sp1_active(
    backend: ProverBackend,
    moongate_endpoint: &Option<String>,
) -> ActiveSp1Prover {
    match backend {
        ProverBackend::Cuda => {
            // sp1-sdk 6.x dropped the moongate arrangement entirely: sp1-cuda
            // no longer accepts an external prover-server URL (nor spins up a
            // docker image). It talks to a local `sp1-gpu-server` binary over
            // a unix socket at /tmp/sp1-cuda-{device}.sock, spawning it from
            // ~/.sp1/bin/sp1-gpu-server (and downloading it from GitHub when
            // absent or version-mismatched; the contemplant image vendors it
            // so deployments never hit the download path). The config field
            // is still parsed so operators get this explanation instead of a
            // serde error.
            let cuda_prover = match moongate_endpoint {
                Some(endpoint) => {
                    let error_msg = format!(
                        "moongate_endpoint ({endpoint}) is not supported on sp1-sdk 6.x: \
                         external moongate prover servers no longer exist. The CUDA prover now \
                         drives a local sp1-gpu-server on this machine; remove moongate_endpoint \
                         from the SP1 [[provers]] entry (the GPU must be attached to this host)."
                    );
                    error!("{error_msg}");
                    panic!("{error_msg}");
                }
                None => {
                    info!("Building CudaProver (local sp1-gpu-server)...");
                    Arc::new(ProverClient::builder().cuda().build().await)
                }
            };
            ActiveSp1Prover::Cuda(cuda_prover)
        }
        ProverBackend::Cpu => {
            info!("Building SP1 CpuProver...");
            let cpu_prover = Arc::new(ProverClient::builder().cpu().build().await);
            ActiveSp1Prover::Cpu(cpu_prover)
        }
    }
}
