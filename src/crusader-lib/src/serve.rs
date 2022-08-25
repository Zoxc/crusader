use bytes::BytesMut;
use futures::{pin_mut, select, FutureExt};
use parking_lot::Mutex;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::{sync::Arc, time::Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{
    channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};
use tokio::sync::oneshot;
use tokio::task::{self, yield_now};
use tokio::{join, signal, time};
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};

use crate::protocol::{self, codec, receive, send, ClientMessage, LatencyMeasure, ServerMessage};

use futures::stream::StreamExt;
use std::{io, thread};

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

#[derive(Debug)]
struct SlotUpdate {
    slot: u64,
    client: Option<Arc<Client>>,
    reply: Option<oneshot::Sender<()>>,
}

#[derive(Debug)]
struct Client {
    ip: Ipv6Addr,
    tx_message: UnboundedSender<ServerMessage>,
    tx_latency: Sender<LatencyMeasure>,
    rx_latency: Mutex<Receiver<LatencyMeasure>>,
    overload: AtomicBool,
}

impl Client {
    fn forward_latency_msgs(&self) {
        let mut rx = self.rx_latency.lock();

        let mut measures = Vec::new();

        while let Ok(measure) = rx.try_recv() {
            measures.push(measure);
        }

        self.tx_message
            .send(ServerMessage::LatencyMeasures(measures))
            .ok();
    }
}

struct State {
    started: Instant,
    dummy_data: Vec<u8>,
    clients: Mutex<Vec<Option<Arc<Client>>>>,
    pong_v6: UnboundedSender<SlotUpdate>,
    pong_v4: Option<UnboundedSender<SlotUpdate>>,
    msg: Box<dyn Fn(&str) + Send + Sync>,
}

fn ip_to_ipv6_mapped(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(ip) => ip.to_ipv6_mapped(),
        IpAddr::V6(ip) => ip,
    }
}

pub struct OnDrop<F: Fn()>(pub F);

impl<F: Fn()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
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
    let mut _client_dropper = None;

    loop {
        let request: ClientMessage = receive(&mut stream_rx).await?;
        match request {
            ClientMessage::NewClient => {
                (state.msg)(&format!("Serving {}, version {}", addr, hello.version));

                let client = {
                    let client = {
                        let mut clients = state.clients.lock();
                        let free_slot =
                            clients.iter_mut().enumerate().find(|slot| slot.1.is_none());

                        free_slot.map(|(slot, data)| {
                            let (tx_message, rx_message) = unbounded_channel();

                            let (tx_latency, rx_latency) = channel(100);
                            let slot = slot as u64;
                            let new_client = Arc::new(Client {
                                ip: ip_to_ipv6_mapped(addr.ip()),
                                tx_message,
                                tx_latency,
                                rx_latency: Mutex::new(rx_latency),
                                overload: AtomicBool::new(false),
                            });
                            *data = Some(new_client.clone());

                            receiver = Some(rx_message);
                            client = Some(new_client.clone());
                            (slot, new_client)
                        })
                    };

                    if let Some((slot, client)) = client {
                        // Update IPv6 pong
                        let (rx, tx) = oneshot::channel();
                        state.pong_v6.send(SlotUpdate {
                            slot,
                            client: Some(client.clone()),
                            reply: Some(rx),
                        })?;
                        tx.await.ok();

                        // Update IPv4 pong
                        let tx = state.pong_v4.as_ref().map(|pong_v4| {
                            let (reply, tx) = oneshot::channel();
                            pong_v4
                                .send(SlotUpdate {
                                    slot,
                                    client: Some(client.clone()),
                                    reply: Some(reply),
                                })
                                .ok();
                            tx
                        });
                        if let Some(tx) = tx {
                            tx.await.ok();
                        }

                        let state = state.clone();
                        _client_dropper = Some(move || {
                            state
                                .pong_v6
                                .send(SlotUpdate {
                                    slot,
                                    client: None,
                                    reply: None,
                                })
                                .ok();
                            state.pong_v4.as_ref().map(|rx| {
                                rx.send(SlotUpdate {
                                    slot,
                                    client: None,
                                    reply: None,
                                })
                                .ok()
                            });
                        });

                        Some(slot)
                    } else {
                        None
                    }
                };

                send(&mut stream_tx, &ServerMessage::NewClient(client)).await?;
            }
            ClientMessage::Associate(id) => {
                client = Some(
                    state
                        .clients
                        .lock()
                        .get(id as usize)
                        .and_then(|client| client.as_ref())
                        .cloned()
                        .and_then(|client| (client.ip == addr.ip()).then_some(client))
                        .ok_or("Unable to assoicate client")?,
                );
            }
            ClientMessage::GetMeasurements => {
                let receiver = receiver.as_mut().ok_or("Not the main client")?;

                let client = client.clone().ok_or("Not the main client")?;

                let done = Arc::new(AtomicBool::new(false));
                let done_ = done.clone();

                let get_pings = tokio::spawn(async move {
                    let mut interval = time::interval(Duration::from_millis(20));
                    loop {
                        interval.tick().await;

                        client.forward_latency_msgs();

                        if done_.load(Ordering::Acquire) {
                            return client.overload.load(Ordering::SeqCst);
                        }
                    }
                });

                loop {
                    let message = {
                        let request = receive::<_, ClientMessage, _>(&mut stream_rx).fuse();
                        pin_mut!(request);

                        let message = receiver.recv().fuse();
                        pin_mut!(message);

                        select! {
                            request = request => {
                                match request? {
                                    ClientMessage::StopMeasurements => None,
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
                        done.store(true, Ordering::Release);
                        let overload = get_pings.await?;

                        // Send pending messages
                        while let Ok(message) = receiver.try_recv() {
                            send(&mut stream_tx, &message).await?;
                        }

                        send(
                            &mut stream_tx,
                            &ServerMessage::MeasurementsDone { overload },
                        )
                        .await?;
                        break;
                    }
                }
            }
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
                            .tx_message
                            .send(ServerMessage::Measure {
                                stream: test_stream,
                                time: current_time
                                    .saturating_duration_since(state.started)
                                    .as_micros() as u64,
                                bytes: current_bytes,
                            })
                            .ok();

                        if done_.load(Ordering::Acquire) {
                            client
                                .tx_message
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
            ClientMessage::StopMeasurements => {
                return Err("Unexpected StopMeasurements".into());
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

async fn handle_ping(
    state: &State,
    slots: &[Option<Arc<Client>>],
    packet: &[u8],
    src: SocketAddr,
    socket: &UdpSocket,
) {
    let valid_ping = bincode::deserialize(packet)
        .ok()
        .and_then(|ping: protocol::Ping| {
            slots
                .get(ping.id as usize)
                .and_then(|client| client.as_ref())
                .and_then(|client| {
                    (ip_to_ipv6_mapped(src.ip()) == client.ip).then_some((client, ping))
                })
        });

    if let Some((client, ping)) = valid_ping {
        let time = Instant::now()
            .saturating_duration_since(state.started)
            .as_micros() as u64;

        let measure = LatencyMeasure {
            time,
            index: ping.index,
        };

        if client.tx_latency.try_send(measure).is_err() {
            client.overload.store(true, Ordering::SeqCst);
        }

        socket
            .send_to(packet, &src)
            .await
            .map_err(|error| {
                (state.msg)(&format!("Unable to reply to UDP ping: {:?}", error));
            })
            .ok();
    }
}

async fn pong(socket: UdpSocket, state: Arc<State>, mut rx: UnboundedReceiver<SlotUpdate>) {
    let mut slots: Vec<_> = (0..SLOTS).map(|_| None).collect();
    let mut buf = [0; 128];

    loop {
        let packet = {
            let socket_packet = socket.recv_from(&mut buf).fuse();
            pin_mut!(socket_packet);

            let message = rx.recv().fuse();
            pin_mut!(message);

            select! {
                result = socket_packet => {
                    match result {
                        Ok((len, src)) => {
                            Some((len, src))
                        }
                        Err(error) => {
                            (state.msg)(&format!("Unable to get UDP ping: {:?}", error));
                            None
                        }
                    }
                },
                slot_update = message => {
                    slot_update.map(|slot_update| {
                        slots[slot_update.slot as usize] = slot_update.client;
                        slot_update.reply.map(|reply| reply.send(()).ok());
                    });
                    None
                },
            }
        };

        if let Some((len, src)) = packet {
            let packet = &mut buf[..len];
            handle_ping(&state, slots.as_slice(), packet, src, &socket).await;
        }
    }
}

const SLOTS: usize = 1000;

async fn serve_async(
    port: u16,
    msg: Box<dyn Fn(&str) + Send + Sync>,
) -> Result<(), Box<dyn Error>> {
    let socket_v6 =
        UdpSocket::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)).await?;
    let socket_v4 = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
        .await
        .map_err(|err| (msg)(&format!("Failed to bind IPv4 UDP: {}", err)))
        .ok();

    let (pong_ipv6_tx, pong_ipv6_rx) = unbounded_channel();
    let (pong_ipv4_tx, pong_ipv4_rx) = unbounded_channel();

    let state = Arc::new(State {
        started: Instant::now(),
        dummy_data: crate::test::data(),
        clients: Mutex::new((0..SLOTS).map(|_| None).collect()),
        pong_v6: pong_ipv6_tx,
        pong_v4: socket_v4.as_ref().map(|_| pong_ipv4_tx),
        msg,
    });

    let v6 = TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)).await?;
    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
        .await
        .map_err(|err| (state.msg)(&format!("Failed to bind IPv4 TCP: {}", err)))
        .ok();

    tokio::spawn(pong(socket_v6, state.clone(), pong_ipv6_rx));

    socket_v4.map(|socket| {
        tokio::spawn(pong(socket, state.clone(), pong_ipv4_rx));
    });

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
