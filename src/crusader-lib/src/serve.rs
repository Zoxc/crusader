use bytes::BytesMut;
use futures::{pin_mut, select, FutureExt};
use parking_lot::Mutex;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::{self, yield_now, JoinHandle};
use tokio::{join, signal, time};
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};

use crate::protocol::{self, codec, receive, send, ClientMessage, ServerMessage};

use futures::stream::StreamExt;
use std::future::Future;
use std::task::{Context, Poll};
use std::{io, thread};

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
    message: UnboundedSender<ServerMessage>,
}

struct State {
    dummy_data: Vec<u8>,
    clients: Mutex<HashMap<u64, Arc<Client>>>,
    msg: Box<dyn Fn(&str) + Send + Sync>,
}

async fn client(state: Arc<State>, stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let addr = stream.peer_addr()?;

    let (rx, tx) = stream.into_split();
    let mut stream_rx = FramedRead::new(rx, codec());
    let mut stream_tx = FramedWrite::new(tx, codec());

    let hello = protocol::Hello::new();

    let client_hello: protocol::Hello = receive(&mut stream_rx).await?;

    send(&mut stream_tx, &hello).await?;

    if hello != client_hello {
        (state.msg)(&format!(
            "Client {} had invalid hello {:?}, expected {:?}",
            addr, client_hello, hello
        ));
        return Ok(());
    }

    let mut client = None;
    let mut receiver = None;

    loop {
        let request: ClientMessage = receive(&mut stream_rx).await?;
        match request {
            ClientMessage::NewClient => {
                (state.msg)(&format!("Serving {}, version {}", addr, hello.version));

                let (tx, rx) = unbounded_channel();

                receiver = Some(rx);

                let client = Arc::new(Client {
                    created: Instant::now(),
                    message: tx,
                });
                let id = Arc::as_ptr(&client) as u64;
                state.clients.lock().insert(id, client);
                send(&mut stream_tx, &ServerMessage::NewClient(id)).await?;
            }
            ClientMessage::Associate(id) => {
                client = Some(
                    state
                        .clients
                        .lock()
                        .get(&id)
                        .cloned()
                        .ok_or("Unable to assoicate client")?,
                );
            }
            ClientMessage::GetMeasurements => loop {
                let message = {
                    let request = receive::<_, ClientMessage, _>(&mut stream_rx).fuse();
                    pin_mut!(request);

                    let message = receiver
                        .as_mut()
                        .ok_or("Not the main client")?
                        .recv()
                        .fuse();
                    pin_mut!(message);

                    select! {
                        request = request => {
                            match request? {
                                ClientMessage::Done => None,
                                _ => {
                                    Err("Closed early")?
                                }
                            }
                        },
                        message = message => message,
                    }
                };

                if let Some(message) = message {
                    send(&mut stream_tx, &message).await?;
                } else {
                    send(&mut stream_tx, &ServerMessage::MeasurementsDone).await?;
                    (state.msg)(&format!("Serving complete for {}", addr));
                    return Ok(());
                }
            },
            ClientMessage::LoadFromServer => {
                let raw = stream_tx.get_mut();

                let done = Arc::new(AtomicBool::new(false));
                let done_ = done.clone();

                let complete = async move {
                    let request = receive::<_, ClientMessage, _>(&mut stream_rx)
                        .await
                        .map_err(|err| err.to_string())?;

                    match request {
                        ClientMessage::Done => (),
                        _ => Err("Closed early")?,
                    }

                    done.store(true, Ordering::Release);

                    Ok::<(), String>(())
                };

                let write = async move {
                    loop {
                        raw.write_all(state.dummy_data.as_ref())
                            .await
                            .map_err(|err| err.to_string())?;

                        if done_.load(Ordering::Acquire) {
                            break;
                        }

                        yield_now().await;
                    }
                    Ok::<(), String>(())
                };

                let (a, b) = join!(complete, write);

                a?;
                b?;

                return Ok(());
            }
            ClientMessage::LoadFromClient {
                stream: test_stream,
                bandwidth_interval,
            } => {
                let client = client.ok_or("No associated client")?;

                let mut raw =
                    FramedRead::with_capacity(stream_rx.into_inner(), CountingCodec, 512 * 1024);

                let bytes = Arc::new(AtomicU64::new(0));
                let bytes_ = bytes.clone();
                let done = Arc::new(AtomicBool::new(false));
                let done_ = done.clone();

                tokio::spawn(async move {
                    let mut interval = time::interval(Duration::from_micros(bandwidth_interval));
                    loop {
                        interval.tick().await;

                        let current_time = Instant::now();
                        let current_bytes = bytes_.load(Ordering::Acquire);

                        client
                            .message
                            .send(ServerMessage::Measure {
                                stream: test_stream,
                                time: current_time.duration_since(client.created).as_micros()
                                    as u64,
                                bytes: current_bytes,
                            })
                            .ok();

                        if done_.load(Ordering::Acquire) {
                            client
                                .message
                                .send(ServerMessage::MeasureStreamDone {
                                    stream: test_stream,
                                })
                                .ok();
                            break;
                        }
                    }
                });

                while let Some(size) = raw.next().await {
                    let size = size?;
                    bytes.fetch_add(size as u64, Ordering::Release);
                    yield_now().await;
                }

                done.store(true, Ordering::Release);

                return Ok(());
            }
            ClientMessage::Done => {
                (state.msg)(&format!("Serving complete for {}", addr));

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
                    client(state.clone(), socket).await.map_err(|error| {
                        (state.msg)(&format!("Error from client {}: {}", addr, error));
                    })
                });
            }
            Err(error) => {
                (state.msg)(&format!("Error accepting client: {}", error));
            }
        }
    }
}

async fn pong(state: Arc<State>, addr: SocketAddr) -> Result<JoinHandle<()>, Box<dyn Error>> {
    let socket = UdpSocket::bind(addr).await?;

    Ok(tokio::spawn(async move {
        let mut buf = [0; 256];

        loop {
            let result = socket.recv_from(&mut buf).await;

            match result {
                Ok((len, src)) => {
                    let buf = &mut buf[..len];
                    socket
                        .send_to(buf, &src)
                        .await
                        .map_err(|error| {
                            (state.msg)(&format!("Unable to reply to UDP ping: {:?}", error));
                        })
                        .ok();
                }
                Err(error) => {
                    (state.msg)(&format!("Unable to get UDP ping: {:?}", error));
                }
            }
        }
    }))
}

async fn serve_async(
    port: u16,
    msg: Box<dyn Fn(&str) + Send + Sync>,
) -> Result<(), Box<dyn Error>> {
    let state = Arc::new(State {
        dummy_data: crate::test::data(),
        clients: Mutex::new(HashMap::new()),
        msg,
    });

    let v6 = TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)).await?;
    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
        .await
        .map_err(|err| (state.msg)(&format!("Failed to bind IPv4 TCP: {}", err)))
        .ok();

    pong(
        state.clone(),
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
    )
    .await?;
    pong(
        state.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
    )
    .await
    .map_err(|err| (state.msg)(&format!("Failed to bind IPv4 UDP: {}", err)))
    .ok();

    task::spawn(listen(state.clone(), v6));
    v4.map(|v4| task::spawn(listen(state.clone(), v4)));

    (state.msg)("Server running...");

    Ok(())
}

pub fn serve_until(
    port: u16,
    msg: Box<dyn Fn(&str) + Send + Sync>,
    started: Box<dyn FnOnce(Result<(), String>) + Send>,
    done: Box<dyn FnOnce() + Send>,
) -> oneshot::Sender<()> {
    let (tx, rx) = oneshot::channel();

    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            match serve_async(port, msg).await {
                Ok(()) => {
                    started(Ok(()));
                    rx.await.unwrap();
                }
                Err(error) => started(Err(error.to_string())),
            }
        });

        done();
    });

    tx
}

pub fn serve(port: u16) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        serve_async(
            port,
            Box::new(|msg: &str| {
                let msg = msg.to_owned();
                task::spawn_blocking(move || println!("{msg}"));
            }),
        )
        .await
        .unwrap();
        signal::ctrl_c().await.unwrap();
        println!("Server aborting...");
    });
}
