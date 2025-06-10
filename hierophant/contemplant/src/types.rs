use alloy_primitives::B256;
use log::error;
use log::{Log, Metadata, Record, info};
use network_lib::ContemplantProofStatus;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

// datastructure to store only <max_proofs_stored> proofs because they can be very big an expensive
// to store in memory.  In most cases, we only need to stored the most recent proof in memory, but
// there are some edge cases where the last 2-3 are needed.
pub struct ProofStore {
    max_proofs_stored: usize,
    //proofs: HashMap<B256, ContemplantProofStatus>,
    proofs: Vec<(B256, ContemplantProofStatus)>,
    current_proof_index: usize,
}

impl ProofStore {
    pub fn new(max_proofs_stored: usize) -> Self {
        if max_proofs_stored < 1 {
            let error_msg = format!("Contemplant's config max_proofs_stored must be > 1");
            error!("{error_msg}");
            panic!("{error_msg}");
        }
        // initialize proofs to be size <max_proofs_stored>
        let default_proof = (B256::default(), ContemplantProofStatus::default());
        let proofs = vec![default_proof; max_proofs_stored];
        Self {
            max_proofs_stored,
            proofs,
            current_proof_index: 0,
        }
    }

    // increments the next index to insert a proof by 1, looping back to the start of the vector if
    // we're at the end
    fn increment_current_proof_index(&mut self) {
        // increment by 1, looping to the start if its at capacity (proof_size)
        let new_proof_index = (self.current_proof_index + 1) % self.max_proofs_stored;
        self.current_proof_index = new_proof_index;
    }

    // overwrite the current index with the new proof status, then increment the current index
    pub fn insert(&mut self, request_id: B256, proof_status: ContemplantProofStatus) {
        match self
            .proofs
            .iter()
            .enumerate()
            .find(|(_, (this_request_id, _))| this_request_id == &request_id)
        {
            // if this request_id is already in the list, overwrite its index
            Some((index, _)) => self.proofs[index] = (request_id, proof_status),
            None => {
                self.proofs[self.current_proof_index] = (request_id, proof_status);
                self.increment_current_proof_index();
            }
        }
    }

    // yes I know, this is O(n) when it could be O(1) with a hash map BUT in practice
    // it is much faster because self.proofs.length is <5 and we don't
    // have to hash anything
    pub fn get(&self, request_id: &B256) -> Option<&ContemplantProofStatus> {
        match self
            .proofs
            .iter()
            .find(|(this_request_id, _)| this_request_id == request_id)
        {
            Some((_, proof_status)) => Some(proof_status),
            None => None,
        }
    }

    pub fn get_mut(&mut self, request_id: &B256) -> Option<&mut ContemplantProofStatus> {
        match self
            .proofs
            .iter_mut()
            .find(|(this_request_id, _)| this_request_id == request_id)
        {
            Some((_, proof_status)) => Some(proof_status),
            None => None,
        }
    }
}

// Custom logger wrapper
pub struct LogCapturingWrapper {
    buffer: Arc<Mutex<VecDeque<String>>>,
    capture_enabled: Arc<Mutex<bool>>,
}

impl LogCapturingWrapper {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::new())),
            capture_enabled: Arc::new(Mutex::new(false)),
        }
    }

    pub fn enable_capture(&self) {
        *self.capture_enabled.lock().unwrap() = true;
    }

    pub fn disable_capture(&self) {
        *self.capture_enabled.lock().unwrap() = false;
    }

    pub fn get_captured_logs(&self) -> Vec<String> {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.drain(..).collect()
    }
}

impl Log for LogCapturingWrapper {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if *self.capture_enabled.lock().unwrap() {
            // When capturing, enable info+ logs regardless of RUST_LOG
            metadata.level() <= log::Level::Info
        } else {
            // When not capturing, respect env_logger settings
            let temp_logger = env_logger::Builder::from_default_env().build();
            temp_logger.enabled(metadata)
        }
    }

    fn log(&self, record: &Record) {
        // Always send to env_logger for normal console output (respects RUST_LOG)
        let temp_logger = env_logger::Builder::from_default_env().build();
        temp_logger.log(record);

        // Capture when enabled, regardless of RUST_LOG
        if *self.capture_enabled.lock().unwrap() && record.level() <= log::Level::Info {
            let formatted = format!("{} - {}", record.level(), record.args());
            self.buffer.lock().unwrap().push_back(formatted);
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_proofs(x: usize) -> Vec<(B256, ContemplantProofStatus)> {
        let mut proofs = Vec::new();
        for _ in 0..x {
            proofs.push((B256::random(), ContemplantProofStatus::unexecuted()));
        }
        proofs
    }

    #[test]
    fn test_init_proof_store() {
        let store = ProofStore::new(1);
        assert_eq!(store.proofs.len(), 1);

        let store = ProofStore::new(50);
        assert_eq!(store.proofs.len(), 50);
    }

    #[test]
    fn test_insert_proof() {
        let proof = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        store.insert(proof.0, proof.1.clone());

        let proof_status = store.get(&proof.0).cloned();

        assert_eq!(proof_status, Some(proof.1));

        for new_proof in generate_proofs(2) {
            store.insert(new_proof.0, new_proof.1.clone());
        }

        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, None);
    }

    #[test]
    fn test_increment_index() {
        let proof = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        assert_eq!(store.current_proof_index, 0);
        store.insert(proof.0, proof.1.clone());
        assert_eq!(store.current_proof_index, 1);
        // overwrite that proof status
        store.insert(proof.0, ContemplantProofStatus::default());
        assert_eq!(store.current_proof_index, 1);

        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, Some(ContemplantProofStatus::default()));

        for new_proof in generate_proofs(3) {
            store.insert(new_proof.0, new_proof.1.clone());
        }

        assert_eq!(store.current_proof_index, 0);

        // previous proof doesnt exist anymore
        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, None);
    }

    /*
    #[test]
    fn test_realistic_insert_pattern() {
        let proof_a = (B256::random(), ContemplantProofStatus::unexecuted());
        let proof_a_executed = (
            proof_a.0,
            ContemplantProofStatus::proof_complete(vec![], 12),
        );

        let proof_b = (B256::random(), ContemplantProofStatus::unexecuted());
        let proof_b_executed = (
            proof_b.0,
            ContemplantProofStatus::proof_complete(vec![], 12),
        );

        let proof_c = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        assert_eq!(store.current_proof_index, 0);

        store.insert(proof_a.0, proof_a.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_a.0), Some(&proof_a.1));

        store.insert(proof_a_executed.0, proof_a_executed.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_b.0, proof_b.1.clone());
        assert_eq!(store.current_proof_index, 0);
        assert_eq!(store.get(&proof_b.0), Some(&proof_b.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_b_executed.0, proof_b_executed.1.clone());
        assert_eq!(store.current_proof_index, 0);
        assert_eq!(store.get(&proof_b_executed.0), Some(&proof_b_executed.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_c.0, proof_c.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_c.0), Some(&proof_c.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_b_executed.0), Some(&proof_b_executed.1));

        // but proof_a should have been pushed out by now
        assert_eq!(store.get(&proof_a_executed.0), None);
    }
    */
}
