use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::protocol;
use crate::protocol::RawLatency;

// Note that rmp_serde doesn't not use an enumerator when serializing Option.
// Be careful about which types are inside Option.

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Elasped {
    pub microseconds: u64,
}

// V0 specific

#[derive(Serialize, Deserialize)]
pub struct RawPingV0 {
    pub index: u64,
    pub sent: Duration,
    pub latency: Option<Duration>,
}

impl RawPingV0 {
    pub fn to_v1(&self) -> RawPing {
        RawPing {
            index: self.index,
            sent: self.sent,
            latency: self.latency.map(|total| RawLatency {
                total: Some(total),
                up: Duration::from_secs(0),
            }),
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
            generated_by: String::new(),
            config: self.config.to_v1(),
            start: self.start,
            server_latency: Duration::from_secs(0),
            ipv6: false,
            duration: self.duration,
            stream_groups: self.stream_groups.clone(),
            pings: self.pings.iter().map(|ping| ping.to_v1()).collect(),
            server_overload: false,
            load_termination_timeout: false,
            peer_pings: None,
            test_data: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TestKind {
    Download,
    Upload,
    Bidirectional,
}

impl TestKind {
    pub fn name(&self) -> &'static str {
        match *self {
            Self::Download => "Download",
            Self::Upload => "Upload",
            Self::Bidirectional => "Bidirectional",
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TestData {
    pub start: Duration,
    pub end: Duration,
    pub kind: TestKind,
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RawPing {
    pub index: u64,
    pub sent: Duration,
    pub latency: Option<RawLatency>,
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
            version: 2,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RawResult {
    pub version: u64,
    pub generated_by: String,
    pub config: RawConfig,
    pub ipv6: bool,
    #[serde(default)]
    pub load_termination_timeout: bool, // Added in V2
    #[serde(default)]
    pub server_overload: bool, // Added in V2
    pub server_latency: Duration,
    pub start: Duration,
    pub duration: Duration,
    pub stream_groups: Vec<RawStreamGroup>,
    pub pings: Vec<RawPing>,
    #[serde(default)]
    pub peer_pings: Option<Vec<RawPing>>, // Added in V2
    #[serde(default)] // Added in V2
    pub test_data: Vec<TestData>,
}

impl RawResult {
    pub fn streams(&self) -> u64 {
        self.stream_groups
            .first()
            .map(|group| group.streams.len())
            .unwrap_or_default()
            .try_into()
            .unwrap()
    }

    pub fn download(&self) -> bool {
        self.stream_groups
            .iter()
            .any(|group| group.download && !group.both)
    }

    pub fn upload(&self) -> bool {
        self.stream_groups
            .iter()
            .any(|group| !group.download && !group.both)
    }

    pub fn idle(&self) -> bool {
        self.stream_groups.is_empty()
    }

    pub fn both(&self) -> bool {
        self.stream_groups.iter().any(|group| group.both)
    }

    pub fn load_from_reader(reader: impl Read) -> Option<Self> {
        let mut file = BufReader::new(reader);
        let header: RawHeader = bincode::deserialize_from(&mut file).ok()?;
        if header.magic != RawHeader::default().magic {
            return None;
        }
        match header.version {
            0 => {
                let result: RawResultV0 = bincode::deserialize_from(file).ok()?;
                Some(result.to_v1())
            }
            1 | 2 => {
                let data = snap::read::FrameDecoder::new(file);
                Some(rmp_serde::decode::from_read(data).ok()?)
            }
            _ => None,
        }
    }

    pub fn load(path: &Path) -> Option<Self> {
        Self::load_from_reader(File::open(path).ok()?)
    }

    pub fn save_to_writer(&self, writer: impl Write) -> Result<(), anyhow::Error> {
        let mut file = BufWriter::new(writer);

        bincode::serialize_into(&mut file, &RawHeader::default())?;

        let mut compressor = snap::write::FrameEncoder::new(file);

        self.serialize(&mut rmp_serde::Serializer::new(&mut compressor).with_struct_map())?;

        compressor.flush()?;
        Ok(())
    }

    pub fn save(&self, name: &Path) -> Result<(), anyhow::Error> {
        self.save_to_writer(File::create(name)?)
    }
}
