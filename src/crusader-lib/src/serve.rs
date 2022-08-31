use futures::{pin_mut, select, FutureExt};
use parking_lot::Mutex;
use socket2::{Domain, Protocol, Socket};
use std::collections::HashMap;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpSocket, TcpStream, UdpSocket};
use tokio::sync::mpsc::{
    channel, unbounded_channel, Receiver, Sender, UnboundedReceiver, UnboundedSender,
};
use tokio::sync::{oneshot, watch};
use tokio::task::{self};
use tokio::{signal, time, time::Instant};
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::protocol::{
    self, codec, receive, send, ClientMessage, LatencyMeasure, ServerMessage, TestStream,
};
use crate::test;

use std::thread;

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
    loads: Mutex<HashMap<u32, watch::Sender<Option<Instant>>>>,
    uploads: Mutex<HashMap<TestStream, Arc<AtomicBool>>>,
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

    fn load_waiter(&self, group: u32) -> watch::Receiver<Option<Instant>> {
        self.loads
            .lock()
            .entry(group)
            .or_insert_with(|| watch::channel(None).0)
            .subscribe()
    }

    async fn schedule_loads(
        &self,
        state: &State,
        groups: Vec<u32>,
        delay: u64,
    ) -> Result<ServerMessage, Box<dyn Error>> {
        let time = Instant::now() + Duration::from_micros(delay);
        {
            let loads = self.loads.lock();
            for group in &groups {
                loads.get(group).ok_or("Unknown group")?.send(Some(time))?;
            }
        }

        Ok(ServerMessage::ScheduledLoads {
            groups,
            time: time.saturating_duration_since(state.started).as_micros() as u64,
        })
    }
}

struct State {
    started: Instant,
    dummy_data: Vec<u8>,
    clients: Mutex<Vec<Option<Arc<Client>>>>,
    pong_v6: UnboundedSender<SlotUpdate>,
    pong_v4: UnboundedSender<SlotUpdate>,
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

    let mut buffer = Vec::with_capacity(512 * 1024);
    buffer.extend((0..buffer.capacity()).map(|_| 0));

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

                            let (tx_latency, rx_latency) = channel(200);
                            let slot = slot as u64;
                            let new_client = Arc::new(Client {
                                ip: ip_to_ipv6_mapped(addr.ip()),
                                tx_message,
                                tx_latency,
                                rx_latency: Mutex::new(rx_latency),
                                overload: AtomicBool::new(false),
                                loads: Mutex::new(HashMap::new()),
                                uploads: Mutex::new(HashMap::new()),
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
                        let (rx, tx) = oneshot::channel();
                        state
                            .pong_v4
                            .send(SlotUpdate {
                                slot,
                                client: Some(client.clone()),
                                reply: Some(rx),
                            })
                            .ok();
                        tx.await.ok();

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
                            state
                                .pong_v4
                                .send(SlotUpdate {
                                    slot,
                                    client: None,
                                    reply: None,
                                })
                                .ok();
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
                        .and_then(|client| {
                            (client.ip == ip_to_ipv6_mapped(addr.ip())).then_some(client)
                        })
                        .ok_or("Unable to assoicate client")?,
                );
            }
            ClientMessage::GetMeasurements => {
                let receiver = receiver.as_mut().ok_or("Not the main client")?;

                let client = client.clone().ok_or("Not the main client")?;
                let client_ = client.clone();

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
                            request = request => Err(request?),
                            message = message => Ok(message),
                        }
                    };

                    match message {
                        Ok(Some(message)) => {
                            send(&mut stream_tx, &message).await?;
                        }
                        Ok(None) | Err(ClientMessage::StopMeasurements) => {
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
                        Err(ClientMessage::LoadComplete { stream }) => {
                            println!("upload done {:?}", stream);
                            let done = client_
                                .uploads
                                .lock()
                                .get(&stream)
                                .ok_or("Expected upload stream")?
                                .clone();
                            tokio::spawn(async move {
                                time::sleep(test::LOAD_EXIT_DELAY).await;
                                done.store(true, Ordering::Release);
                            });
                        }
                        Err(ClientMessage::ScheduleLoads { groups, delay }) => {
                            let reply = client_.schedule_loads(&state, groups, delay).await?;
                            send(&mut stream_tx, &reply).await?;
                        }
                        Err(msg) => {
                            return Err(
                                format!("Unexpected message during measurement {:?}", msg).into()
                            )
                        }
                    }
                }
            }

            ClientMessage::LoadFromServer {
                stream: test_stream,
                duration,
                delay,
            } => {
                let client = client.ok_or("No associated client")?;

                // TODO: Wait for the socket to become readable for the dummy byte

                send(&mut stream_tx, &ServerMessage::WaitingForLoad).await?;

                let stream = stream_tx
                    .into_inner()
                    .reunite(stream_rx.into_inner())
                    .unwrap();

                let mut waiter = client.load_waiter(test_stream.group);
                waiter.changed().await?;
                let start = waiter.borrow().ok_or("Expected time")? + Duration::from_micros(delay);

                time::sleep_until(start).await;

                test::write_data(
                    stream,
                    state.dummy_data.as_ref(),
                    start + Duration::from_micros(duration),
                )
                .await?;

                client
                    .tx_message
                    .send(ServerMessage::LoadComplete {
                        stream: test_stream,
                    })
                    .ok();

                return Ok(());
            }
            ClientMessage::LoadFromClient {
                stream: test_stream,
                duration,
                delay,
                bandwidth_interval,
            } => {
                let client = client.ok_or("No associated client")?;

                send(&mut stream_tx, &ServerMessage::WaitingForLoad).await?;

                let mut stream = stream_rx
                    .into_inner()
                    .reunite(stream_tx.into_inner())
                    .unwrap();

                stream.write_u8(1).await.unwrap();

                let reading_done = Arc::new(AtomicBool::new(false));

                client
                    .uploads
                    .lock()
                    .insert(test_stream, reading_done.clone());

                let bytes = Arc::new(AtomicU64::new(0));
                let bytes_ = bytes.clone();
                let (done_tx, mut done_rx) = oneshot::channel();

                let mut waiter = client.load_waiter(test_stream.group);
                waiter.changed().await?;
                let start = waiter.borrow().ok_or("Expected time")? + Duration::from_micros(delay);

                time::sleep_until(start).await;

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

                        if let Ok(timeout) = done_rx.try_recv() {
                            client
                                .tx_message
                                .send(ServerMessage::MeasureStreamDone {
                                    stream: test_stream,
                                    timeout,
                                })
                                .ok();
                            break;
                        }
                    }
                });

                let timeout = test::read_data(
                    stream,
                    &mut buffer,
                    &bytes,
                    start + Duration::from_micros(duration),
                    reading_done,
                )
                .await?;

                println!("reading done {:?} timeout:{:?}", test_stream, timeout);

                done_tx
                    .send(timeout)
                    .map_err(|_| "Unable to signal reading completion")?;

                return Ok(());
            }
            ClientMessage::Done => {
                (state.msg)(&format!("Serving complete for {}", addr));

                return Ok(());
            }
            msg @ (ClientMessage::StopMeasurements
            | ClientMessage::ScheduleLoads { .. }
            | ClientMessage::LoadComplete { .. }) => {
                return Err(format!("Unexpected message {:?}", msg).into());
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
    let socket_v6 = Socket::new(Domain::IPV6, socket2::Type::DGRAM, Some(Protocol::UDP))?;
    socket_v6.set_only_v6(true)?;
    socket_v6.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port).into())?;
    let socket_v6: std::net::UdpSocket = socket_v6.into();
    socket_v6.set_nonblocking(true)?;
    let socket_v6 = UdpSocket::from_std(socket_v6)?;

    let socket_v4 =
        UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)).await?;

    let (pong_ipv6_tx, pong_ipv6_rx) = unbounded_channel();
    let (pong_ipv4_tx, pong_ipv4_rx) = unbounded_channel();

    let state = Arc::new(State {
        started: Instant::now(),
        dummy_data: crate::test::data(),
        clients: Mutex::new((0..SLOTS).map(|_| None).collect()),
        pong_v6: pong_ipv6_tx,
        pong_v4: pong_ipv4_tx,
        msg,
    });

    let v6 = Socket::new(Domain::IPV6, socket2::Type::STREAM, Some(Protocol::TCP))?;
    v6.set_only_v6(true)?;
    let v6: std::net::TcpStream = v6.into();
    v6.set_nonblocking(true)?;
    let v6 = TcpSocket::from_std_stream(v6);
    v6.bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))?;
    let v6 = v6.listen(1024)?;

    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;

    tokio::spawn(pong(socket_v6, state.clone(), pong_ipv6_rx));
    tokio::spawn(pong(socket_v4, state.clone(), pong_ipv4_rx));

    task::spawn(listen(state.clone(), v6));
    task::spawn(listen(state.clone(), v4));

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
