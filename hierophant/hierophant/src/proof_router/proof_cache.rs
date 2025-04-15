use crate::VkHash;
use anyhow::{Context, Result};
use log::{error, info};
use std::{fs, path::Path};

// If the proving server goes offline and it has completed some proofs, when it comes back up
// it will have to re-run those exact same proofs.  Proofs can take hours so we want some degree of
// persistence for when the server binary is stopped & started.  At the very least for easier
// debugging.

// This cache writes proofs to disk, using a LRU cache (more appropriately, Least Recently
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
            info!(
                "{proof_cache_directory} directory doesn't exist.  Creating it for caching completed proofs."
            );
            fs::create_dir(path).context("Create {proof_cache_directory} directory")?;
        } else {
            info!("Found {proof_cache_directory} directory.");
        }

        let mut cache_list = Vec::with_capacity(cache_size);
        // fill with default values so we never have to check if current_cache_index is out of
        // bounds
        cache_list.resize_with(cache_size, || (VkHash::default(), "empty".into()));

        Ok(Self {
            cache_size,
            proof_cache_directory: format!("{proof_cache_directory}"),
            current_cache_index: 0,
            cache_list,
        })
    }

    // Retreive a proof that we previously computed.  This can save us hours of proving time.
    // Safe to call even if we're not sure we have a proof.
    // called in get_proof_status()
    pub async fn read_proof(&self, vk_hash: &VkHash) -> Option<Vec<u8>> {
        // if cache is disabled
        if self.cache_size == 0 {
            return None;
        }

        let proof_path_name = vk_hash_to_file_path(&self.proof_cache_directory, &vk_hash);
        let proof_path = Path::new(&proof_path_name);
        if proof_path.exists() {
            info!(
                "Found proof with vk_hash {} on disk!  Loading from file {} ",
                vk_hash.to_hex_string(),
                proof_path_name
            );

            // load proof from file and return
            match fs::read(proof_path) {
                Ok(v) => Some(v),
                Err(e) => {
                    error!("Error reading proof {proof_path_name} from disk: {e}");
                    None
                }
            }
        } else {
            None
        }

        // we haven't received this proof request yet, it's not in the proof_request_lookup
    }

    // writes the completed proof to file, deleting the Least Recently Completed proof that the
    // cache is aware of.
    // Takes a mutable reference, so only use this if you're sure the proof isn't already on disk
    pub async fn write_proof(&mut self, proof_bytes: Vec<u8>, vk_hash: &VkHash) -> Result<()> {
        // if cache is disabled
        if self.cache_size == 0 {
            return Ok(());
        }

        info!(
            "Writing proof 0x{} to disk.  Num bytes: {}",
            vk_hash.to_hex_string(),
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

        // get proof file name so we can write
        let proof_path_name = vk_hash_to_file_path(&self.proof_cache_directory, vk_hash);
        let path = Path::new(&proof_path_name);

        // write completed proof to file
        fs::write(path, proof_bytes).context(format!(
            "Write proof id {} to file {}",
            vk_hash.to_hex_string(),
            proof_path_name
        ))?;

        // overwrite the current_cache_index in the cache_list vector with our newly written proof
        let new_elem = (vk_hash.clone(), proof_path_name);
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

fn vk_hash_to_file_path(proof_cache_directory: &str, vk_hash: &VkHash) -> String {
    format!("{}/{}", proof_cache_directory, vk_hash.to_hex_string())
}
