use crate::VkHash;
use alloy_primitives::B256;
use anyhow::{Context, Result, anyhow};
use log::{error, info};
use std::{collections::HashMap, fs, path::Path};

// If the proving server goes offline and it has completed some span proofs, when it comes back up
// it will have to re-run those exact same spans.  Spans can take hours so we want some degree of
// persistence for when the server binary is stopped & started.  At the very least for easier
// debugging.

// This cache writes span proofs to disk, using a LRU cache (more appropriately, Least Recently
// Completed Proof).
#[derive(Debug)]
pub struct ProofCache {
    // max cache size.  Setting it to 0 disables the cache
    cache_size: usize,
    // directory where proofs are saved
    proof_cache_directory: String,
    // goes up to cache_size and loops.  Keeps track of the next proof to replace
    // increments each time we write a proof to disk
    current_cache_index: usize,
    // when we get a new proof, replace the proof at current_index and also look it up in
    // proof_requests and evict the address that we just replaced
    // (proof_id, proof_file_path_name)
    cache_list: Vec<(VkHash, String)>,
}

impl ProofCache {
    pub fn new(cache_size: usize, proof_cache_directory: &str) -> Result<Self> {
        // Create `proofs/` directory if it doesn't already exist
        let path = Path::new(proof_cache_directory);
        if !path.exists() {
            info!("{proof_cache_directory} directory doesn't exist.  Creating it for caching completed proofs.");
            fs::create_dir(path).context("Create {proof_cache_directory} directory")?;
        } else {
            info!("Found {proof_cache_directory} directory.");
        }

        let mut cache_list = Vec::with_capacity(cache_size);
        // fill with default values so we never have to check if current_cache_index is out of
        // bounds
        cache_list.resize_with(cache_size, || (B256::default(), "empty".into()));

        Ok(Self {
            cache_size,
            proof_cache_directory: format!("{proof_cache_directory}"),
            current_cache_index: 0,
            cache_list,
        })
    }

    // If we have a proof locally we can save hours of time by skipping span proof generation.
    // This just returns true if it does indeed exist locally.
    // Careful: Just because it exists doesn't mean it is known by proof_request_lookup.  This is
    // guarded against by returning a proof_id to the proposer and subsequently calling record_proof_request
    // even when we already have the proof locally
    // Called in `request_span_proof`
    pub fn proof_exists(&self, proof_request: &SpanProofRequest) -> bool {
        // if cache is disabled
        if self.cache_size == 0 {
            return false;
        }

        let proof_path_name =
            proof_request_to_file_path(&self.proof_cache_directory, proof_request);
        let proof_path = Path::new(&proof_path_name);

        proof_path.exists()
    }

    // Retreive a proof that we previously computed.  This can save us hours of proving time.
    // Safe to call even if we're not sure we have a proof.
    // called in get_proof_status()
    pub async fn read_proof(&self, proof_id: &B256) -> Result<Option<Vec<u8>>> {
        // if cache is disabled
        if self.cache_size == 0 {
            return Ok(None);
        }

        // TODO: get proof
        todo!()

        // TODO: this changes as well.  Proof naming will have to change in the cache
        match maybe_proof {
            Some(GenericProofRequest::Span(proof_request)) => {
                let proof_path_name =
                    proof_request_to_file_path(&self.proof_cache_directory, &proof_request);
                let proof_path = Path::new(&proof_path_name);
                if proof_path.exists() {
                    info!(
                        "Found cached span proof of request {}, loading from file {}",
                        proof_request, proof_path_name
                    );

                    // load proof from file and return
                    let proof_bytes = fs::read(proof_path)
                        .context(format!("Reading proof from file {}", proof_path_name))?;
                    Ok(Some(proof_bytes))
                } else {
                    // This means the proof is requested & the cache is aware of it (its in proof_request_lookup) but it hasn't
                    // completed yet (didn't get written to disk in write_proof())
                    Ok(None)
                }
            }
            Some(GenericProofRequest::Agg(_)) => {
                // we never save agg proofs to the cache, skip
                Ok(None)
            }
            // we haven't received this proof request yet, it's not in the proof_request_lookup
            None => Ok(None),
        }
    }

    // writes the completed proof to file, deleting the Least Recently Completed proof that the
    // cache is aware of.
    // Takes a mutable reference, so only use this if you're sure the proof isn't already on disk
    pub async fn write_proof(&mut self, proof_bytes: Vec<u8>, proof_id: &B256) -> Result<()> {
        // if cache is disabled
        if self.cache_size == 0 {
            return Ok(());
        }

        info!(
            "num proof bytes in write proof in cache: {}",
            proof_bytes.len()
        );

        // overwrite the current_cache_index of cache_list
        let elem = self
            .cache_list
            .get_mut(self.current_cache_index)
            .context(format!(
                "Index {} out of bounds of cache_list vector",
                self.current_cache_index,
            ))?;

        // if a proof exists here, evict it by deleting the file
        let old_proof_path = Path::new(&elem.1);
        if old_proof_path.exists() {
            fs::remove_file(old_proof_path)
                .context(format!("Delete old proof file {:?}", old_proof_path))?;
        }

        // get proof request parameters (needed for proof file name) from the proof_id
        let proof_request = match self
            .proof_request_cache_client
            .lookup_proof_request(proof_id)
            .await
        {
            Ok(Some(GenericProofRequest::Span(span))) => span,
            Ok(None) => {
                error!("Couldn't find span proof {proof_id} in proof request cache");
                return Err(anyhow!(
                    "Couldn't find span proof {proof_id} in proof request cache"
                ));
            }
            Ok(Some(GenericProofRequest::Agg(_))) => {
                // we don't write agg proofs.  Just skip
                return Ok(());
            }
            Err(e) => return Err(anyhow!(e)),
        };

        // get proof file name so we can write
        let proof_path_name =
            proof_request_to_file_path(&self.proof_cache_directory, &proof_request);
        let path = Path::new(&proof_path_name);

        // write completed proof to file
        fs::write(path, proof_bytes).context(format!(
            "Write proof id {} to file {}",
            proof_id, proof_path_name
        ))?;

        // overwrite the current_cache_index in the cache_list vector with our newly written proof
        let new_elem = (*proof_id, proof_path_name);
        *elem = new_elem;

        self.increment_current_cache_index();
        Ok(())
    }

    fn increment_current_cache_index(&mut self) {
        if self.cache_size > 0 {
            // increment by 1, looping to the start if its at capacity (cache_size)
            let new_cache_index = (self.current_cache_index + 1) % self.cache_size;
            self.current_cache_index = new_cache_index;
        }
    }
}

fn proof_request_to_file_path(
    proof_cache_directory: &str,
    proof_request: &SpanProofRequest,
) -> String {
    format!(
        "{}/{}-{}",
        proof_cache_directory, proof_request.start, proof_request.end
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constructor() {
        let test_client = ProofRequestCacheClient::new(0);
        let proof_cache = ProofCache::new(0, "".into(), test_client.clone()).unwrap();
        assert_eq!(proof_cache.cache_list.len(), 0);

        let proof_cache = ProofCache::new(10, "".into(), test_client.clone()).unwrap();
        assert_eq!(proof_cache.cache_list.len(), 10);
    }

    #[test]
    fn test_proof_request_to_file_path() {
        let proof_request = SpanProofRequest {
            start: 255,
            end: 256,
        };

        let proof_cache_dir = "proofs";
        let proof_file_path_name = proof_request_to_file_path(proof_cache_dir, &proof_request);
        let correct_proof_file_path_name = format!("{proof_cache_dir}/255-256");

        assert_eq!(proof_file_path_name, correct_proof_file_path_name);
    }

    #[test]
    fn test_increment_current_cache_index_0() {
        let test_client = ProofRequestCacheClient::new(0);
        let mut proof_cache = ProofCache::new(0, "", test_client).unwrap();
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 0);
    }

    #[test]
    fn test_increment_current_cache_index_1() {
        let test_client = ProofRequestCacheClient::new(0);
        let mut proof_cache = ProofCache::new(1, "", test_client).unwrap();
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 0);
    }

    #[test]
    fn test_increment_current_cache_index_2() {
        let test_client = ProofRequestCacheClient::new(0);
        let mut proof_cache = ProofCache::new(2, "", test_client).unwrap();
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 1);
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 0);
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 1);
    }

    #[test]
    fn test_increment_current_cache_index_3() {
        let test_client = ProofRequestCacheClient::new(0);
        let mut proof_cache = ProofCache::new(3, "", test_client).unwrap();
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 1);
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 2);
        proof_cache.increment_current_cache_index();
        assert_eq!(proof_cache.current_cache_index, 0);
    }
}
