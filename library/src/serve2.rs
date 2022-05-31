use bytes::BytesMut;
use parking_lot::Mutex;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::{join, time};
use tokio_util::codec::{Decoder, Framed};

use crate::protocol::{self, codec, receive, send, ClientMessage, ServerMessage};

use futures::stream::StreamExt;
use std::future::Future;
use std::io;
use std::task::{Context, Poll};

struct ExtractPollRead<'a, F: AsyncRead + ?Sized> {
    reader: &'a mut F,
    buf: &'a mut [u8],
}

impl<F> Future for ExtractPollRead<'_, F>
where
    F: AsyncRead + ?Sized,
{
    type Output = Poll<io::Result<usize>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        let reader = unsafe { Pin::new_unchecked(&mut *this.reader) };
        let mut buf = ReadBuf::new(this.buf);
        let poll = reader.poll_read(cx, &mut buf);
        let poll = poll.map(|result| result.map(|_| buf.filled().len()));
        Poll::Ready(poll)
    }
}

pub struct CountingCodec;

impl Decoder for CountingCodec {
    type Item = usize;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<usize>, io::Error> {
        if !buf.is_empty() {
            let len = buf.len();
            buf.clear();
            Ok(Some(len))
        } else {
            Ok(None)
        }
    }
}
/*
impl Encoder<Bytes> for CountingCodec {
    type Error = io::Error;

    fn encode(&mut self, data: Bytes, buf: &mut BytesMut) -> Result<(), io::Error> {
        buf.reserve(data.len());
        buf.put(data);
        Ok(())
    }
}

impl Encoder<BytesMut> for CountingCodec {
    type Error = io::Error;

    fn encode(&mut self, data: BytesMut, buf: &mut BytesMut) -> Result<(), io::Error> {
        buf.reserve(data.len());
        buf.put(data);
        Ok(())
    }
} */

struct Client {
    created: Instant,
}

struct State {
    clients: Mutex<HashMap<u64, Arc<Client>>>,
}

async fn client(state: Arc<State>, stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let addr = stream.peer_addr()?;

    let mut stream = Framed::new(stream, codec());

    let hello = protocol::Hello::new();

    let client_hello: protocol::Hello = receive(&mut stream).await?;

    send(&mut stream, &hello).await?;

    if hello != client_hello {
        println!(
            "Client {} had invalid hello {:?}, expected {:?}",
            addr, client_hello, hello
        );
        return Ok(());
    }

    let mut client = None;

    loop {
        let request: ClientMessage = receive(&mut stream).await?;
        match request {
            ClientMessage::NewClient => {
                println!("Serving {}, version {}", addr, hello.version);

                let client = Arc::new(Client {
                    created: Instant::now(),
                });
                let id = Arc::as_ptr(&client) as u64;
                state.clients.lock().insert(id, client);
                send(&mut stream, &ServerMessage::NewClient(id)).await?;
            }
            ClientMessage::Associate(id) => {
                client = Some(
                    state
                        .clients
                        .lock()
                        .get(&id)
                        .ok_or_else(|| "Unable to assoicate client")?,
                );
            }
            ClientMessage::GetMeasurements => {
                //thread::spawn(f)
            }
            ClientMessage::LoadFromClient => {
                let mut raw =
                    Framed::with_capacity(stream.into_inner(), CountingCodec, 1024 * 1024);

                let start = Instant::now();

                let interval = 1000;

                let bytes = Arc::new(AtomicU64::new(0));
                let bytes2 = bytes.clone();

                tokio::spawn(async move {
                    let mut interval = time::interval(Duration::from_millis(interval));
                    let mut previous = start;
                    let mut data = 0;
                    loop {
                        interval.tick().await;

                        let current_time = Instant::now();
                        let elapsed = current_time.saturating_duration_since(previous);
                        previous = current_time;

                        let current = bytes2.load(Ordering::Relaxed);

                        let delta = current - data;

                        data = current;

                        let mbits = (delta as f64 * 8.0) / 1000.0 / 1000.0;
                        let rate = mbits / elapsed.as_secs_f64();
                        println!("Rate: {:>10.2} Mbps, Bytes: {}", rate, delta);

                        if start.elapsed() > Duration::from_secs(8) {
                            break;
                        }
                    }
                });

                println!("Loading started for {}", addr);

                while let Some(size) = raw.next().await {
                    let size = size?;
                    bytes.fetch_add(size as u64, Ordering::Relaxed);
                }

                println!("Loading complete for {}", addr);

                return Ok(());
            }
            ClientMessage::Done => {
                println!("Serving complete for {}", addr);

                return Ok(());
            }
        };
    }
}

async fn listen(state: Arc<State>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((socket, addr)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    client(state, socket).await.map_err(|error| {
                        println!("Error from client {}: {}", addr, error);
                    })
                });
            }
            Err(error) => {
                println!("Error accepting client: {}", error);
            }
        }
    }
}

async fn pong(addr: SocketAddr) {
    tokio::spawn(async move {
        let socket = UdpSocket::bind(addr).await.unwrap();

        let mut buf = [0; 256];

        loop {
            let (len, src) = socket
                .recv_from(&mut buf)
                .await
                .expect("unable to get udp ping");

            let buf = &mut buf[..len];
            socket
                .send_to(buf, &src)
                .await
                .expect("unable to reply to udp ping");
        }
    });
}

async fn serve_async() {
    let state = Arc::new(State {
        clients: Mutex::new(HashMap::new()),
    });

    let state2 = state.clone();

    let port = 30481;
    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
        .await
        .unwrap();
    let v6 = TcpListener::bind((Ipv6Addr::UNSPECIFIED, port))
        .await
        .unwrap();

    let pong_v4 = pong(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
    let pong_v6 = pong(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port));

    join!(listen(state, v4), listen(state2, v6), pong_v4, pong_v6);
}

pub fn serve() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(serve_async());
}
