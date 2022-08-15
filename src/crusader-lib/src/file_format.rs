use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::protocol;

#[derive(Serialize, Deserialize)]
pub struct RawPoint {
    pub time: Duration,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct RawStream {
    pub data: Vec<RawPoint>,
}

impl RawStream {
    pub(crate) fn to_vec(&self) -> Vec<(u64, u64)> {
        self.data
            .iter()
            .map(|point| (point.time.as_micros() as u64, point.bytes))
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
pub struct RawStreamGroup {
    pub download: bool,
    pub both: bool,
    pub streams: Vec<RawStream>,
}

#[derive(Serialize, Deserialize)]
pub struct RawPing {
    pub index: usize,
    pub sent: Duration,
    pub latency: Option<Duration>,
}

#[derive(Serialize, Deserialize)]
pub struct RawConfig {
    pub load_duration: u64,
    pub grace_duration: u64,
    pub ping_interval: u64,
    pub bandwidth_interval: u64,
}

#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub struct RawHeader {
    pub magic: u64,
    pub version: u64,
}

impl Default for RawHeader {
    fn default() -> Self {
        Self {
            magic: protocol::MAGIC,
            version: 1,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RawResult {
    pub config: RawConfig,
    pub start: Duration,
    pub duration: Duration,
    pub stream_groups: Vec<RawStreamGroup>,
    pub pings: Vec<RawPing>,
}

impl RawResult {
    pub fn load(path: &Path) -> Option<Self> {
        let mut file = BufReader::new(File::open(path).ok()?);
        let header: RawHeader = bincode::deserialize_from(&mut file).ok()?;
        if header.magic != RawHeader::default().magic {
            return None;
        }
        match header.version {
            0 => bincode::deserialize_from(file).ok(),
            1 => {
                let data = snap::read::FrameDecoder::new(file);
                bincode::deserialize_from(data).ok()
            }
            _ => None,
        }
    }

    pub fn save(&self, name: &Path) {
        let mut file = BufWriter::new(File::create(name).unwrap());

        bincode::serialize_into(&mut file, &RawHeader::default()).unwrap();

        let mut compressor = snap::write::FrameEncoder::new(file);

        bincode::serialize_into(&mut compressor, self).unwrap();

        compressor.flush().unwrap();
    }
}
