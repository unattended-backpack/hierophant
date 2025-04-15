use serde::{Deserialize, Serialize};

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub port: usize,
}
