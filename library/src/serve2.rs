use bincode::{deserialize_from, serialize_into};
use parking_lot::Mutex;
use std::error::Error;
use std::{
    collections::HashMap,
    io::{ErrorKind, Read},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::protocol::{self, ClientMessage, ServerMessage};

struct Client {
    created: Instant,
}

struct State {
    clients: Mutex<HashMap<u64, Arc<Client>>>,
}

pub async fn serve_async() {}

pub fn serve() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(serve_async());
}
