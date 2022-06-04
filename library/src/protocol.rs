use std::error::Error;

use bytes::{Bytes, BytesMut};
use futures::{Sink, SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::codec::{length_delimited, LengthDelimitedCodec};

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

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct TestStream {
    pub group: u32,
    pub id: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ServerMessage {
    NewClient(u64),
    Measure {
        stream: TestStream,
        time: u64,
        bytes: u64,
    },
    MeasureStreamDone {
        stream: TestStream,
    },
    MeasurementsDone,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ClientMessage {
    NewClient,
    Associate(u64),
    Done,
    LoadFromClient {
        stream: TestStream,
        bandwidth_interval: u64,
    },
    LoadFromServer,
    GetMeasurements,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Ping {
    pub index: u32,
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
) -> Result<T, Box<dyn Error>>
where
    E: Error + 'static,
{
    let bytes = stream.next().await.ok_or("Expected object")??;
    Ok(bincode::deserialize(&bytes)?)
}
