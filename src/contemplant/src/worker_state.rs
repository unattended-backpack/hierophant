use crate::config::{AssessorConfig, Config, ProverBackend, VmChoice};
use crate::proof_executor::{Risc0Executor, Sp1Executor};
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
    pub proof_store_client: ProofStoreClient,
    pub assessor_config: AssessorConfig,
    // just used for healthcheck.  Is set to true in api/connect_to_hierophant
    pub ready: Arc<Mutex<bool>>,
}

impl WorkerState {
    pub fn new(config: Config) -> Self {
        let mut sp1_executor: Option<Sp1Executor> = None;
        let mut risc0_executor: Option<Risc0Executor> = None;

        for prover_cfg in &config.provers {
            match prover_cfg.vm {
                VmChoice::Sp1 => {
                    let active = build_sp1_active(prover_cfg.backend, &prover_cfg.moongate_endpoint);
                    let mock = Arc::new(ProverClient::builder().mock().build());
                    let progress_tracking_available = matches!(active, ActiveSp1Prover::Cuda(_))
                        && prover_cfg.moongate_endpoint.is_some();
                    if progress_tracking_available {
                        info!("SP1 progress tracking enabled (CUDA + moongate endpoint)");
                    } else {
                        info!("SP1 progress tracking disabled (CPU, dockerized CUDA, or mock)");
                    }
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
            }
        }
        info!("Prover(s) built");

        let proof_store_client = ProofStoreClient::new(config.max_proofs_stored);
        let ready = Arc::new(Mutex::new(false));

        Self {
            sp1_executor,
            risc0_executor,
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
}

fn build_sp1_active(
    backend: ProverBackend,
    moongate_endpoint: &Option<String>,
) -> ActiveSp1Prover {
    match backend {
        ProverBackend::Cuda => {
            #[allow(unreachable_code, unused_variables)]
            let cuda_prover = match moongate_endpoint {
                Some(endpoint) => {
                    #[cfg(not(feature = "enable-native-gnark"))]
                    {
                        let error_msg = "Please rebuild with: `--features enable-native-gnark` or remove moongate_endpoint to use the dockerized CUDA prover";
                        error!("{error_msg}");
                        panic!("{error_msg}");
                    }
                    // sp1-cuda passes the endpoint straight into twirp's
                    // Client::new, which joins the method name onto the base
                    // URL's path. moongate-server's router is mounted at
                    // `/twirp/`, so an operator-supplied `http://host:3000`
                    // produces `http://host:3000/api.ProverService/Ready` and
                    // moongate answers 404 BadRoute, silently looping the
                    // ready poll until the 5-minute SP1 timeout. Normalize by
                    // appending `/twirp/` when the operator didn't already.
                    // (twirp-rs also requires a trailing slash on the base
                    // URL; see twirp::Client::from_base_url.)
                    let normalized = if endpoint.contains("/twirp") {
                        let mut s = endpoint.clone();
                        if !s.ends_with('/') {
                            s.push('/');
                        }
                        s
                    } else {
                        let trimmed = endpoint.trim_end_matches('/');
                        format!("{trimmed}/twirp/")
                    };
                    info!("Building CudaProver with moongate endpoint {normalized}...");
                    Arc::new(ProverClient::builder().cuda().server(&normalized).build())
                }
                None => {
                    #[cfg(feature = "enable-native-gnark")]
                    {
                        let error_msg = "Please rebuild without `--features enable-native-gnark` to use the dockerized CUDA prover or supply a moongate_endpoint";
                        error!("{error_msg}");
                        panic!("{error_msg}");
                    }
                    info!("Starting CudaProver docker container...");
                    Arc::new(ProverClient::builder().cuda().build())
                }
            };
            ActiveSp1Prover::Cuda(cuda_prover)
        }
        ProverBackend::Cpu => {
            info!("Building SP1 CpuProver...");
            let cpu_prover = Arc::new(ProverClient::builder().cpu().build());
            ActiveSp1Prover::Cpu(cpu_prover)
        }
    }
}
