use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::protocol;

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Elasped {
    pub microseconds: u64,
}

// V0 specific

#[derive(Serialize, Deserialize)]
pub struct RawPingV0 {
    pub index: usize,
    pub sent: Duration,
    pub latency: Option<Duration>,
}

impl RawPingV0 {
    pub fn to_v1(&self) -> RawPing {
        RawPing {
            index: self.index,
            sent: self.sent,
            latency: self.latency,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RawConfigV0 {
    // Seconds
    pub load_duration: u64,
    pub grace_duration: u64,

    // Milliseconds
    pub ping_interval: u64,
    pub bandwidth_interval: u64,
}

impl RawConfigV0 {
    pub fn to_v1(&self) -> RawConfig {
        RawConfig {
            stagger: Duration::from_secs(0),
            load_duration: Duration::from_secs(self.load_duration),
            grace_duration: Duration::from_secs(self.grace_duration),
            ping_interval: Duration::from_millis(self.ping_interval),
            bandwidth_interval: Duration::from_millis(self.bandwidth_interval),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RawResultV0 {
    pub config: RawConfigV0,
    pub start: Duration,
    pub duration: Duration,
    pub stream_groups: Vec<RawStreamGroup>,
    pub pings: Vec<RawPingV0>,
}

impl RawResultV0 {
    pub fn to_v1(&self) -> RawResult {
        RawResult {
            version: 0,
            config: self.config.to_v1(),
            start: self.start,
            latency_to_server: Duration::from_secs(0),
            ipv6: false,
            duration: self.duration,
            stream_groups: self.stream_groups.clone(),
            pings: self.pings.iter().map(|ping| ping.to_v1()).collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RawPoint {
    pub time: Duration,
    pub bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
pub struct RawConfig {
    // Microseconds
    pub stagger: Duration,
    pub load_duration: Duration,
    pub grace_duration: Duration,
    pub ping_interval: Duration,
    pub bandwidth_interval: Duration,
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
    pub version: u64,
    pub config: RawConfig,
    pub ipv6: bool,
    pub latency_to_server: Duration,
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
            0 => {
                let result: RawResultV0 = bincode::deserialize_from(file).ok()?;
                Some(result.to_v1())
            }
            1 => {
                let data = snap::read::FrameDecoder::new(file);
                Some(rmp_serde::decode::from_read(data).ok()?)
            }
            _ => None,
        }
    }

    pub fn save(&self, name: &Path) {
        let mut file = BufWriter::new(File::create(name).unwrap());

        bincode::serialize_into(&mut file, &RawHeader::default()).unwrap();

        let mut compressor = snap::write::FrameEncoder::new(file);

        self.serialize(&mut rmp_serde::Serializer::new(&mut compressor).with_struct_map())
            .unwrap();

        compressor.flush().unwrap();
    }
}
