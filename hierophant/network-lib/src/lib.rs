use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub port: usize,
}
