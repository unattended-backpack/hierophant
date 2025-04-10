use serde::{Deserialize, Serialize};

pub const REGISTER_WORKER_ENDPOINT: &str = "register_worker";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub port: usize,
}
