use serde::{Deserialize, Serialize};

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub port: usize,
}

// newtype wrapper for keeping request_id bytes distinct from other Vec<u8>
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct RequestId(Vec<u8>);

impl RequestId {
    pub fn to_hex_string(&self) -> String {
        format!("0x{}", hex::encode(self.clone().0))
    }
}

impl Default for RequestId {
    fn default() -> Self {
        RequestId(vec![])
    }
}

// so we can convert from Vec<u8> to RequestId
impl From<Vec<u8>> for RequestId {
    fn from(bytes: Vec<u8>) -> Self {
        RequestId(bytes)
    }
}

// so we can convert RequestId into Vec<u8>
impl From<RequestId> for Vec<u8> {
    fn from(request_id: RequestId) -> Self {
        request_id.0
    }
}
