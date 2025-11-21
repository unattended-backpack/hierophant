use crate::config::{AssessorConfig, Config, ProverType};
use crate::proof_store::ProofStoreClient;
use log::{error, info};
use sp1_sdk::{CpuProver, CudaProver, ProverClient};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub enum ActiveProver {
    Cuda(Arc<CudaProver>),
    Cpu(Arc<CpuProver>),
}

#[derive(Clone)]
pub struct WorkerState {
    pub active_prover: ActiveProver,
    pub mock_prover: Arc<CpuProver>,
    pub progress_tracking_available: bool,
    pub proof_store_client: ProofStoreClient,
    pub assessor_config: AssessorConfig,
    // just used for healthcheck.  Is set to true in api/connect_to_hierophant
    pub ready: Arc<Mutex<bool>>,
}

impl WorkerState {
    pub fn new(config: Config) -> Self {
        // Build only the prover type that's configured to avoid SP1 errors
        let active_prover = match config.prover_type {
            ProverType::Cuda => {
                // compiler will always complain about one of these branches being unreachable, depending on if
                // you compiled with `features enable-native-gnark` or not
                #[allow(unreachable_code, unused_variables)]
                // If `enable-native-gnark` is set, use the provided moongate CUDA prover endpoint.
                // Otherwise, sp1-sdk spins up a local dockerized version of the moongate CUDA prover.
                let cuda_prover = match &config.moongate_endpoint {
                    // build with undockerized moongate server
                    Some(moongate_endpoint) => {
                        // make sure the `native-gnark` feature is enabled.  Otherwise the contemplant will
                        // error when it tries to finish a GROTH16 proof
                        #[cfg(not(feature = "enable-native-gnark"))]
                        {
                            let error_msg = "Please rebuild with: `--features enable-native-gnark` or remove moongate_endpoint in cargo.toml to use the dockerized CUDA prover";
                            error!("{error_msg}");
                            panic!("{error_msg}");
                        }
                        info!("Building CudaProver with moongate endpoint {moongate_endpoint}...");
                        Arc::new(
                            ProverClient::builder()
                                .cuda()
                                .server(moongate_endpoint)
                                .build(),
                        )
                    }
                    // spin up cuda prover docker container
                    None => {
                        // build using dockerized CUDA prover
                        #[cfg(feature = "enable-native-gnark")]
                        {
                            let error_msg = "Please rebuild without `--features enable-native-gnark` to use the dockerized CUDA prover or supply a moongate_endpoint to contemplant.toml";
                            error!("{error_msg}");
                            panic!("{error_msg}");
                        }
                        info!("Starting CudaProver docker container...");
                        Arc::new(ProverClient::builder().cuda().build())
                    }
                };
                ActiveProver::Cuda(cuda_prover)
            }
            ProverType::Cpu => {
                info!("Building CpuProver...");
                let cpu_prover = Arc::new(ProverClient::builder().cpu().build());
                ActiveProver::Cpu(cpu_prover)
            }
        };

        let mock_prover = Arc::new(ProverClient::builder().mock().build());
        info!("Prover built");

        // Progress tracking is only available when using CUDA with a remote moongate endpoint
        let progress_tracking_available = matches!(active_prover, ActiveProver::Cuda(_))
            && config.moongate_endpoint.is_some();

        if progress_tracking_available {
            info!("Progress tracking enabled (CUDA prover with moongate endpoint)");
        } else {
            info!("Progress tracking disabled (CPU prover, dockerized CUDA, or mock mode)");
        }

        let proof_store_client = ProofStoreClient::new(config.max_proofs_stored);
        let ready = Arc::new(Mutex::new(false));

        Self {
            active_prover,
            mock_prover,
            progress_tracking_available,
            proof_store_client,
            assessor_config: config.assessor.clone(),
            ready,
        }
    }
}
