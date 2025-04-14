use crate::network::Program;
use alloy_primitives::B256;
use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use log::info;
use sp1_sdk::{SP1Stdin, network::proto::artifact::ArtifactType};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ProgramStore {
    // mapping of program uri to program
    pub store: Arc<Mutex<HashMap<Uuid, Program>>>,
}
/*
pub struct Program {
    /// The verification key hash
    pub vk_hash: ::prost::alloc::vec::Vec<u8>,
    /// The verification key
    #[prost(bytes = "vec", tag = "2")]
    pub vk: ::prost::alloc::vec::Vec<u8>,
    /// The program resource identifier
    pub program_uri: ::prost::alloc::string::String,

    /// The owner of the program
    #[prost(bytes = "vec", tag = "5")]
    pub owner: ::prost::alloc::vec::Vec<u8>,

    /// The unix timestamp of when the program was created
    #[prost(uint64, tag = "6")]
    pub created_at: u64,
    /// The optional name of the program
    #[prost(string, optional, tag = "4")]
    pub name: ::core::option::Option<::prost::alloc::string::String>,
}
*/

impl ProgramStore {
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
