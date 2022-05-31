use serde::{Deserialize, Serialize};

pub const MAGIC: u64 = 0x5372ab82ae7c59cb;
pub const VERSION: u64 = 0;

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct Hello {
    magic: u64,
    pub version: u64,
}

impl Hello {
    pub fn new() -> Self {
        Hello {
            magic: MAGIC,
            version: VERSION,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ServerMessage {
    NewClient(u64),
    WaitingOnLoad,
    Measure {},
    MeasurementsDone,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientMessage {
    NewClient,
    Associate(u64),
    Done,
    LoadFromClient,
    GetMeasurements,
}

#[derive(Serialize, Deserialize)]
pub struct Ping {
    pub index: u32,
    pub timestamp: u64,
}
