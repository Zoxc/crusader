use anyhow::anyhow;
use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::error::Error;
use tokio_util::codec::{length_delimited, LengthDelimitedCodec};

use crate::file_format::RawLatency;

pub const PORT: u16 = 35481;

pub const MAGIC: u64 = 0x5372ab82ae7c59cb;
pub const VERSION: u64 = 3;

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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct TestStream {
    pub group: u32,
    pub id: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LatencyMeasure {
    pub time: u64, // In microseconds and in server time
    pub index: u64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PeerLatency {
    pub sent: u64, // In microseconds and in server time
    pub latency: Option<RawLatency>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ServerMessage {
    NewClient(Option<u64>),
    LatencyMeasures(Vec<LatencyMeasure>),
    Measure {
        stream: TestStream,
        time: u64,
        bytes: u64,
    },
    MeasureStreamDone {
        stream: TestStream,
        timeout: bool,
    },
    MeasurementsDone {
        overload: bool,
    },
    LoadComplete {
        stream: TestStream,
    },
    ScheduledLoads {
        groups: Vec<u32>,
        time: u64,
    },
    WaitingForLoad,
    WaitingForByte,
    NewPeer,
    PeerReady {
        server_latency: u64,
    },
    PeerStarted,
    PeerDone {
        overload: bool,
        latencies: Vec<PeerLatency>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientMessage {
    NewClient,
    Associate(u64),
    Done,
    ScheduleLoads {
        groups: Vec<u32>,
        delay: u64,
    },
    LoadFromClient {
        stream: TestStream,
        duration: u64,
        delay: u64,
        throughput_interval: u64,
    },
    LoadFromServer {
        stream: TestStream,
        duration: u64,
        delay: u64,
    },
    LoadComplete {
        stream: TestStream,
    },
    SendByte,
    GetMeasurements,
    StopMeasurements,
    NewPeer {
        server: [u8; 16],
        port: u16,
        ping_interval: u64,
        estimated_duration: u128,
    },
    PeerStart,
    PeerStop,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Ping {
    pub id: u64,
    pub index: u64,
}

pub fn codec() -> LengthDelimitedCodec {
    length_delimited::Builder::new()
        .little_endian()
        .length_field_type::<u64>()
        .new_codec()
}

pub async fn send<S: Sink<Bytes> + Unpin>(
    sink: &mut S,
    value: &impl Serialize,
) -> Result<(), Box<dyn Error>>
where
    S::Error: Error + 'static,
{
    Ok(sink.send(bincode::serialize(value)?.into()).await?)
}

pub async fn receive<S: Stream<Item = Result<BytesMut, E>> + Unpin, T: for<'a> Deserialize<'a>, E>(
    stream: &mut S,
) -> Result<T, anyhow::Error>
where
    E: Error + Send + Sync + 'static,
{
    let bytes = stream.next().await.ok_or(anyhow!("Expected object"))??;
    Ok(bincode::deserialize(&bytes)?)
}
