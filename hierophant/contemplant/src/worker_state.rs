use crate::config::{AssessorConfig, Config};
use crate::proof_store::ProofStoreClient;
use log::{error, info};
use sp1_sdk::{CpuProver, CudaProver, ProverClient};
use std::sync::Arc;

#[derive(Clone)]
pub struct WorkerState {
    pub cuda_prover: Arc<CudaProver>,
    pub mock_prover: Arc<CpuProver>,
    pub proof_store_client: ProofStoreClient,
    pub assessor_config: AssessorConfig,
}

impl WorkerState {
    pub fn new(config: Config) -> Self {
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
        let mock_prover = Arc::new(ProverClient::builder().mock().build());
        info!("Prover built");

        let proof_store_client = ProofStoreClient::new(config.max_proofs_stored);

        Self {
            cuda_prover,
            mock_prover,
            proof_store_client,
            assessor_config: config.assessor.clone(),
        }
    }
}
