use futures::future::FutureExt;
use futures::{pin_mut, select};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::thread;
use std::{
    error::Error,
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio::{join, task};
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{codec, receive, send, ClientMessage, Ping, ServerMessage};
use crate::test::{hello, udp_handle};

type UpdateFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Copy, Clone)]
pub struct Config {
    pub port: u16,
    pub ping_interval: Duration,
}

#[derive(Debug, Copy, Clone)]
pub enum EventKind {
    Sent,
    Timeout,
    AtServer,
    Pong,
}

#[derive(Debug, Copy, Clone)]
pub struct Event {
    pub ping_index: u64,
    pub kind: EventKind,
    pub time: Duration,
}

#[derive(Clone)]
pub struct Point {
    pub pending: bool,
    pub index: u64,
    pub sent: Duration,
    pub total: Option<Duration>,
    pub up: Option<Duration>,
}

#[derive(Debug, Copy, Clone)]
pub enum State {
    Connecting,
    Syncing,
    Monitoring,
}

pub struct Data {
    pub state: Mutex<State>,
    pub start: Instant,
    pub limit: usize,
    pub points: tokio::sync::Mutex<VecDeque<Point>>,
    update_fn: UpdateFn,
}

impl Data {
    pub fn new(limit: usize, update_fn: UpdateFn) -> Self {
        Self {
            state: Mutex::new(State::Connecting),
            start: Instant::now(),
            limit,
            points: tokio::sync::Mutex::new(VecDeque::new()),
            update_fn,
        }
    }
}

async fn test_async(
    config: Config,
    server: &str,
    data: Arc<Data>,
    stop: oneshot::Receiver<()>,
) -> Result<(), Box<dyn Error>> {
    let control = net::TcpStream::connect((server, config.port)).await?;
    control.set_nodelay(true)?;

    let server = control.peer_addr()?;

    *data.state.lock() = State::Syncing;
    (data.update_fn)();

    let (rx, tx) = control.into_split();
    let mut control_rx = FramedRead::new(rx, codec());
    let mut control_tx = FramedWrite::new(tx, codec());

    hello(&mut control_tx, &mut control_rx).await?;

    send(&mut control_tx, &ClientMessage::NewClient).await?;

    let setup_start = data.start;

    let reply: ServerMessage = receive(&mut control_rx).await?;
    let id = match reply {
        ServerMessage::NewClient(Some(id)) => id,
        ServerMessage::NewClient(None) => return Err("Server was unable to create client".into()),
        _ => return Err(format!("Unexpected message {:?}", reply).into()),
    };

    let local_udp = if server.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };

    let mut ping_index = 0;

    let (_latency, server_time_offset, mut control_rx) = measure_latency(
        id,
        &mut ping_index,
        &mut control_tx,
        control_rx,
        server,
        local_udp,
        setup_start,
    )
    .await?;

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let ping_interval = config.ping_interval;

    let (event_tx, mut event_rx) = channel(1000);

    send(&mut control_tx, &ClientMessage::GetMeasurements).await?;

    let event_tx_ = event_tx.clone();
    let measures = tokio::spawn(async move {
        let overload_;

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await.unwrap();
            match reply {
                ServerMessage::LatencyMeasures(measures) => {
                    for measure in measures {
                        event_tx_
                            .send(Event {
                                ping_index: measure.index,
                                kind: EventKind::AtServer,
                                time: Duration::from_micros(
                                    measure.time.wrapping_add(server_time_offset),
                                ),
                            })
                            .await
                            .unwrap();
                    }
                }
                ServerMessage::MeasurementsDone { overload } => {
                    overload_ = overload;
                    break;
                }
                _ => panic!("Unexpected message {:?}", reply),
            };
        }

        overload_
    });

    tokio::spawn(ping_recv(
        event_tx.clone(),
        setup_start,
        udp_socket2.clone(),
    ));

    time::sleep(Duration::from_millis(50)).await;

    *data.state.lock() = State::Monitoring;
    (data.update_fn)();

    tokio::spawn(ping_send(
        event_tx.clone(),
        ping_index,
        id,
        setup_start,
        udp_socket2.clone(),
        ping_interval,
    ));

    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            {
                let mut points = data.points.lock().await;
                let i = points
                    .iter()
                    .enumerate()
                    .find(|r| r.1.index == event.ping_index)
                    .map(|r| r.0);
                match event.kind {
                    EventKind::Sent => {
                        while points.len() > data.limit {
                            points.pop_back();
                        }
                        points.push_front(Point {
                            pending: true,
                            index: event.ping_index,
                            sent: event.time,
                            up: None,
                            total: None,
                        });
                    }
                    EventKind::AtServer => {
                        i.map(|i| {
                            points[i].up = Some(event.time.saturating_sub(points[i].sent));
                        });
                    }
                    EventKind::Pong => {
                        i.map(|i| {
                            points[i].pending = false;
                            points[i].total = Some(event.time.saturating_sub(points[i].sent));
                        });
                    }
                    EventKind::Timeout => {
                        i.map(|i| {
                            points[i].pending = false;
                        });
                    }
                }
            }
            (data.update_fn)();
        }
    });

    stop.await?;

    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;
    send(&mut control_tx, &ClientMessage::Done).await?;

    let _server_overload = measures.await?;

    Ok(())
}

async fn measure_latency(
    id: u64,
    ping_index: &mut u64,
    mut control_tx: &mut FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    mut control_rx: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    server: SocketAddr,
    local_udp: SocketAddr,
    setup_start: Instant,
) -> Result<
    (
        Duration,
        u64,
        FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    ),
    Box<dyn Error>,
> {
    send(&mut control_tx, &ClientMessage::GetMeasurements).await?;

    let latencies = tokio::spawn(async move {
        let mut latencies = Vec::new();

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await.unwrap();
            match reply {
                ServerMessage::LatencyMeasures(measures) => {
                    latencies.extend(measures.into_iter());
                }
                ServerMessage::MeasurementsDone { .. } => break,
                _ => panic!("Unexpected message {:?}", reply),
            };
        }

        (latencies, control_rx)
    });

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let samples = 50;

    let ping_start_index = *ping_index;
    let ping_send = tokio::spawn(ping_measure_send(
        ping_start_index,
        id,
        setup_start,
        udp_socket,
        samples,
    ));

    let ping_recv = tokio::spawn(ping_measure_recv(setup_start, udp_socket2, samples));

    let (sent, recv) = join!(ping_send, ping_recv);

    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;

    let (mut latencies, control_rx) = latencies.await?;

    let (sent, new_ping_index) = sent.unwrap();
    *ping_index = new_ping_index;
    let mut recv = recv.unwrap();

    latencies.sort_by_key(|d| d.index);
    recv.sort_by_key(|d| d.0.index);
    let mut pings: Vec<(Duration, Duration, u64)> = sent
        .into_iter()
        .enumerate()
        .filter_map(|(index, sent)| {
            let index = index as u64 + ping_start_index;
            let latency = latencies
                .binary_search_by_key(&index, |e| e.index)
                .ok()
                .map(|ping| latencies[ping].time);

            latency.and_then(|time| {
                recv.binary_search_by_key(&index, |e| e.0.index)
                    .ok()
                    .map(|ping| (sent, recv[ping].1 - sent, time))
            })
        })
        .collect();
    if pings.is_empty() {
        return Err("Unable to measure latency to server".into());
    }

    pings.sort_by_key(|d| d.1);

    let (sent, latency, server_time) = pings[pings.len() / 2];

    let server_pong = sent + latency / 2;

    let server_offset = (server_pong.as_micros() as u64).wrapping_sub(server_time);

    Ok((latency, server_offset, control_rx))
}

async fn ping_measure_send(
    mut index: u64,
    id: u64,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    samples: u32,
) -> (Vec<Duration>, u64) {
    let mut storage = Vec::with_capacity(samples as usize);
    let mut buf = [0; 64];

    let mut interval = time::interval(Duration::from_millis(10));

    for _ in 0..samples {
        interval.tick().await;

        let current = setup_start.elapsed();

        let ping = Ping { id, index };

        index += 1;

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping).unwrap();
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        socket.send(buf).await.unwrap();

        storage.push(current);
    }

    (storage, index)
}

async fn ping_measure_recv(
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    samples: u32,
) -> Vec<(Ping, Duration)> {
    let mut storage = Vec::with_capacity(samples as usize);
    let mut buf = [0; 64];

    let end = time::sleep(Duration::from_millis(10) * samples + Duration::from_millis(1000)).fuse();
    pin_mut!(end);

    loop {
        let result = {
            let packet = socket.recv(&mut buf).fuse();
            pin_mut!(packet);

            select! {
                result = packet => result,
                _ = end => break,
            }
        };

        let current = setup_start.elapsed();
        let len = result.unwrap();
        let buf = &mut buf[..len];
        let ping: Ping = bincode::deserialize(buf).unwrap();

        storage.push((ping, current));
    }

    storage
}

async fn ping_send(
    event_tx: Sender<Event>,
    mut ping_index: u64,
    id: u64,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
) {
    let mut buf = [0; 64];

    let mut interval = time::interval(interval);

    loop {
        interval.tick().await;

        let current = setup_start.elapsed();

        let ping = Ping {
            id,
            index: ping_index,
        };

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping).unwrap();
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        udp_handle(socket.send(buf).await.map(|_| ())).expect("unable to udp ping");

        event_tx
            .send(Event {
                ping_index,
                kind: EventKind::Sent,
                time: current,
            })
            .await
            .unwrap();

        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            time::sleep(Duration::from_secs(1)).await;
            event_tx
                .send(Event {
                    ping_index,
                    kind: EventKind::Timeout,
                    time: Duration::from_secs(0),
                })
                .await
                .unwrap();
        });

        ping_index += 1;
    }
}

async fn ping_recv(
    event_tx: Sender<Event>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
) -> Vec<(Ping, Duration)> {
    let mut buf = [0; 64];

    loop {
        let result = socket.recv(&mut buf).await;

        let current = setup_start.elapsed();
        let len = result.unwrap();
        let buf = &mut buf[..len];
        let ping: Ping = bincode::deserialize(buf).unwrap();

        event_tx
            .send(Event {
                ping_index: ping.index,
                kind: EventKind::Pong,
                time: current,
            })
            .await
            .unwrap();
    }
}

pub fn test_callback(
    config: Config,
    host: &str,
    data: Arc<Data>,
    done: Box<dyn FnOnce(Option<Result<(), String>>) + Send>,
) -> oneshot::Sender<()> {
    let (stop_tx, stop_rx) = oneshot::channel();
    let (force_stop_tx, force_stop_rx) = oneshot::channel();
    let host = host.to_string();
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();

        done(rt.block_on(async move {
            let (tx, rx) = oneshot::channel();
            task::spawn(async move {
                stop_rx.await.ok();
                tx.send(()).ok();
                time::sleep(Duration::from_secs(5)).await;
                force_stop_tx.send(()).ok();
            });

            let mut result = task::spawn(async move {
                test_async(config, &host, data, rx)
                    .await
                    .map_err(|error| error.to_string())
            })
            .fuse();

            select! {
                result = result => {
                    Some(result.map_err(|error| error.to_string()).and_then(|result| result))
                },
                result = force_stop_rx.fuse() => {
                    result.unwrap();
                    None
                },
            }
        }));
    });
    stop_tx
}
