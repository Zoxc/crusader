use bytes::{Bytes, BytesMut};
use futures::future::FutureExt;
use futures::{pin_mut, select, Sink, Stream};
use futures::{stream, StreamExt};
use parking_lot::Mutex;
use rand::prelude::StdRng;
use rand::Rng;
use rand::SeedableRng;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::{
    error::Error,
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::join;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::{oneshot, watch, Semaphore};
use tokio::task::{self, yield_now, JoinHandle};
use tokio::time::Instant;
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::file_format::{
    RawConfig, RawHeader, RawLatency, RawPing, RawPoint, RawResult, RawStream, RawStreamGroup,
};
use crate::peer::connect_to_peer;
use crate::plot::save_graph;
use crate::protocol::{
    codec, receive, send, ClientMessage, Hello, Ping, ServerMessage, TestStream,
};
use crate::serve::OnDrop;

pub(crate) type Msg = Arc<dyn Fn(&str) + Send + Sync>;

const MEASURE_DELAY: Duration = Duration::from_millis(50);

#[derive(PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord)]
pub(crate) enum TestState {
    Setup,
    Grace1,
    LoadFromClient,
    Grace2,
    LoadFromServer,
    Grace3,
    LoadFromBoth,
    Grace4,
    End,
    EndPingRecv,
}

#[derive(Debug)]
struct ScheduledLoads {
    time: Instant,
}

struct State {
    downloads: Mutex<HashMap<TestStream, oneshot::Sender<()>>>,
    timeout: AtomicBool,
}

pub(crate) fn data() -> Vec<u8> {
    let mut vec = Vec::with_capacity(128 * 1024);
    let mut rng = StdRng::from_seed([
        18, 141, 186, 158, 195, 76, 244, 56, 219, 131, 65, 128, 250, 63, 228, 44, 233, 34, 9, 51,
        13, 72, 230, 131, 223, 240, 124, 77, 103, 238, 103, 186,
    ]);
    for _ in 0..vec.capacity() {
        vec.push(rng.gen())
    }
    vec
}

async fn hello_combined<S: Sink<Bytes> + Stream<Item = Result<BytesMut, S::Error>> + Unpin>(
    stream: &mut S,
) -> Result<(), Box<dyn Error>>
where
    S::Error: Error + 'static,
{
    let hello = Hello::new();

    send(stream, &hello).await?;
    let server_hello: Hello = receive(stream).await?;

    if hello != server_hello {
        panic!(
            "Mismatched server hello, got {:?}, expected {:?}",
            server_hello, hello
        );
    }

    Ok(())
}

pub(crate) async fn hello<
    T: Sink<Bytes> + Unpin,
    R: Stream<Item = Result<BytesMut, RE>> + Unpin,
    RE,
>(
    tx: &mut T,
    rx: &mut R,
) -> Result<(), Box<dyn Error>>
where
    T::Error: Error + 'static,
    RE: Error + 'static,
{
    let hello = Hello::new();

    send(tx, &hello).await?;
    let server_hello: Hello = receive(rx).await?;

    if hello != server_hello {
        return Err(format!(
            "Mismatched server hello, got {:?}, expected {:?}",
            server_hello, hello
        )
        .into());
    }

    Ok(())
}

pub(crate) async fn write_data(
    stream: TcpStream,
    data: &[u8],
    until: Instant,
) -> Result<(), Box<dyn Error>> {
    stream.set_nodelay(false).ok();
    stream.set_linger(Some(Duration::from_secs(0))).ok();

    let done = Arc::new(AtomicBool::new(false));
    let done_ = done.clone();

    tokio::spawn(async move {
        time::sleep_until(until).await;
        done.store(true, Ordering::Release);
    });

    loop {
        if let Ok(Err(err)) = time::timeout(Duration::from_millis(50), stream.writable()).await {
            if err.kind() == std::io::ErrorKind::ConnectionReset
                || err.kind() == std::io::ErrorKind::ConnectionAborted
            {
                break;
            } else {
                return Err(err.into());
            }
        }

        if done_.load(Ordering::Acquire) {
            break;
        }
        match stream.try_write(data) {
            Ok(_) => (),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::WouldBlock {
                } else if err.kind() == std::io::ErrorKind::ConnectionReset
                    || err.kind() == std::io::ErrorKind::ConnectionAborted
                {
                    break;
                } else {
                    return Err(err.into());
                }
            }
        }

        yield_now().await;
    }

    std::mem::drop(stream);

    Ok(())
}

pub(crate) async fn read_data(
    stream: TcpStream,
    buffer: &mut [u8],
    bytes: Arc<AtomicU64>,
    until: Instant,
    writer_done: oneshot::Receiver<()>,
) -> Result<bool, Box<dyn Error>> {
    stream.set_linger(Some(Duration::from_secs(0))).ok();

    let reading_done = Arc::new(AtomicBool::new(false));

    // Set `reading_done` to true 2 minutes after the load should terminate.
    let reading_done_ = reading_done.clone();
    tokio::spawn(async move {
        time::sleep_until(until + Duration::from_secs(120)).await;
        reading_done_.store(true, Ordering::Release);
    });

    // Set `reading_done` to true after 5 seconds of not receiving data.
    let reading_done_ = reading_done.clone();
    let bytes_ = bytes.clone();
    tokio::spawn(async move {
        writer_done.await.ok();

        let mut current = bytes_.load(Ordering::Acquire);
        let mut i = 0;
        loop {
            time::sleep(Duration::from_millis(100)).await;

            if reading_done_.load(Ordering::Acquire) {
                break;
            }

            let now = bytes_.load(Ordering::Acquire);

            if now != current {
                i = 0;
                current = now;
            } else {
                i += 1;

                if i > 50 {
                    reading_done_.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });

    // Set `reading_done` to true on exit to terminate the spawned task.
    let reading_done_ = reading_done.clone();
    let _on_drop = OnDrop(|| {
        reading_done_.store(true, Ordering::Release);
    });

    loop {
        if let Ok(Err(err)) = time::timeout(Duration::from_millis(50), stream.readable()).await {
            if err.kind() == std::io::ErrorKind::ConnectionReset
                || err.kind() == std::io::ErrorKind::ConnectionAborted
            {
                return Ok(false);
            } else {
                return Err(err.into());
            }
        }

        loop {
            if reading_done.load(Ordering::Acquire) {
                return Ok(true);
            }

            match stream.try_read(buffer) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    bytes.fetch_add(n as u64, Ordering::Release);
                    yield_now().await;
                }
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::WouldBlock {
                        break;
                    } else if err.kind() == std::io::ErrorKind::ConnectionReset
                        || err.kind() == std::io::ErrorKind::ConnectionAborted
                    {
                        return Ok(false);
                    } else {
                        return Err(err.into());
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct PlotConfig {
    pub split_bandwidth: bool,
    pub transferred: bool,
    pub max_bandwidth: Option<u64>,
    pub max_latency: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub title: Option<String>,
}

#[derive(Copy, Clone)]
pub struct Config {
    pub download: bool,
    pub upload: bool,
    pub both: bool,
    pub port: u16,
    pub load_duration: Duration,
    pub grace_duration: Duration,
    pub streams: u64,
    pub stream_stagger: Duration,
    pub ping_interval: Duration,
    pub bandwidth_interval: Duration,
}

async fn test_async(
    config: Config,
    server: &str,
    latency_peer_server: Option<&str>,
    msg: Msg,
) -> Result<RawResult, Box<dyn Error>> {
    let control = net::TcpStream::connect((server, config.port)).await?;
    control.set_nodelay(true)?;

    let server = control.peer_addr()?;

    msg(&format!("Connected to server {}", server));

    let (rx, tx) = control.into_split();
    let mut control_rx = FramedRead::new(rx, codec());
    let mut control_tx = FramedWrite::new(tx, codec());

    hello(&mut control_tx, &mut control_rx).await?;

    send(&mut control_tx, &ClientMessage::NewClient).await?;

    let setup_start = Instant::now();

    let reply: ServerMessage = receive(&mut control_rx).await?;
    let id = match reply {
        ServerMessage::NewClient(Some(id)) => id,
        ServerMessage::NewClient(None) => return Err("Server was unable to create client".into()),
        _ => return Err(format!("Unexpected message {:?}", reply).into()),
    };

    let loading_streams: u32 = config.streams.try_into().unwrap();

    let grace = config.grace_duration;
    let load_duration = config.load_duration;
    let ping_interval = config.ping_interval;

    let loads = config.both as u32 + config.download as u32 + config.upload as u32;

    let estimated_duration = load_duration * loads + grace * 2;

    let mut peer = if let Some(peer) = latency_peer_server {
        Some(connect_to_peer(config, server, peer, estimated_duration, msg.clone()).await?)
    } else {
        None
    };

    let local_udp = if server.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };

    let mut ping_index = 0;

    let (latency, server_time_offset, mut control_rx) = measure_latency(
        id,
        &mut ping_index,
        &mut control_tx,
        control_rx,
        server,
        local_udp,
        setup_start,
    )
    .await?;

    msg(&format!(
        "Latency to server {:.2} ms",
        latency.as_secs_f64() * 1000.0
    ));

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let data = Arc::new(data());

    let state = Arc::new(State {
        downloads: Mutex::new(HashMap::new()),
        timeout: AtomicBool::new(false),
    });

    let (state_tx, state_rx) = watch::channel((TestState::Setup, setup_start));

    let all_loaders = Arc::new(Semaphore::new(0));
    let mut loader_count = 0;

    let (upload_done_tx, mut upload_done_rx) = channel(config.streams as usize);

    if config.upload {
        loader_count += config.streams;
        upload_loaders(
            all_loaders.clone(),
            id,
            server,
            0,
            config,
            Duration::ZERO,
            data.clone(),
            state_rx.clone(),
            TestState::LoadFromClient,
            upload_done_tx.clone(),
        );
    }

    if config.both {
        loader_count += config.streams;
        upload_loaders(
            all_loaders.clone(),
            id,
            server,
            1,
            config,
            config.stream_stagger / 2,
            data.clone(),
            state_rx.clone(),
            TestState::LoadFromBoth,
            upload_done_tx.clone(),
        );
    }

    let download = config.download.then(|| {
        loader_count += config.streams;
        download_loaders(
            state.clone(),
            all_loaders.clone(),
            id,
            server,
            2,
            config,
            setup_start,
            state_rx.clone(),
            TestState::LoadFromServer,
        )
    });

    let both_download = config.both.then(|| {
        loader_count += config.streams;
        download_loaders(
            state.clone(),
            all_loaders.clone(),
            id,
            server,
            3,
            config,
            setup_start,
            state_rx.clone(),
            TestState::LoadFromBoth,
        )
    });

    send(&mut control_tx, &ClientMessage::GetMeasurements).await?;

    // Wait for all loaders to setup
    let _ = all_loaders.acquire_many(loader_count as u32).await?;

    let upload_semaphore = Arc::new(Semaphore::new(0));
    let upload_semaphore_ = upload_semaphore.clone();
    let both_upload_semaphore = Arc::new(Semaphore::new(0));
    let both_upload_semaphore_ = both_upload_semaphore.clone();

    let (scheduled_load_tx, mut scheduled_load_rx) = channel(4);

    let state_ = state.clone();
    let measures = tokio::spawn(async move {
        let mut bandwidth = Vec::new();
        let mut latencies = Vec::new();
        let overload_;

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await.unwrap();
            match reply {
                ServerMessage::MeasureStreamDone { stream, timeout } => {
                    if timeout {
                        state_.timeout.store(true, Ordering::SeqCst);
                    }

                    if stream.group == 0 {
                        upload_semaphore_.add_permits(1);
                    } else if stream.group == 1 {
                        both_upload_semaphore_.add_permits(1);
                    }
                }
                ServerMessage::Measure {
                    stream,
                    time,
                    bytes,
                } => {
                    bandwidth.push((stream, time, bytes));
                }
                ServerMessage::LatencyMeasures(measures) => {
                    latencies.extend(measures.into_iter());
                }
                ServerMessage::MeasurementsDone { overload } => {
                    overload_ = overload;
                    break;
                }
                ServerMessage::LoadComplete { stream } => {
                    state_
                        .downloads
                        .lock()
                        .remove(&stream)
                        .unwrap()
                        .send(())
                        .unwrap();
                }
                ServerMessage::ScheduledLoads { groups: _, time } => {
                    let time = Duration::from_micros(time.wrapping_add(server_time_offset));
                    scheduled_load_tx
                        .send(ScheduledLoads {
                            time: setup_start + time,
                        })
                        .await
                        .unwrap()
                }
                _ => panic!("Unexpected message {:?}", reply),
            };
        }

        (latencies, bandwidth, overload_)
    });

    if let Some(peer) = peer.as_mut() {
        peer.start().await?;
    }

    let ping_start_index = ping_index;
    let ping_send = tokio::spawn(ping_send(
        ping_index,
        id,
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
        estimated_duration,
    ));

    let ping_recv = tokio::spawn(ping_recv(
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
        estimated_duration,
    ));

    time::sleep(Duration::from_millis(50)).await;

    let start = Instant::now();

    state_tx.send((TestState::Grace1, start)).unwrap();
    time::sleep(grace).await;

    let load_delay = (Duration::from_millis(50) + latency).as_micros() as u64;

    if let Some((semaphore, _)) = download.as_ref() {
        send(
            &mut control_tx,
            &ClientMessage::ScheduleLoads {
                groups: vec![2],
                delay: load_delay,
            },
        )
        .await?;
        let load = scheduled_load_rx.recv().await.unwrap();
        state_tx
            .send((TestState::LoadFromServer, load.time))
            .unwrap();
        msg(&format!("Testing download..."));
        let _ = semaphore.acquire_many(loading_streams).await.unwrap();

        state_tx.send((TestState::Grace2, Instant::now())).unwrap();
        time::sleep(grace).await;
    }

    if config.upload {
        send(
            &mut control_tx,
            &ClientMessage::ScheduleLoads {
                groups: vec![0],
                delay: load_delay,
            },
        )
        .await?;
        let load = scheduled_load_rx.recv().await.unwrap();
        state_tx
            .send((TestState::LoadFromClient, load.time))
            .unwrap();
        msg(&format!("Testing upload..."));

        for _ in 0..config.streams {
            let stream = upload_done_rx.recv().await.ok_or("Expected stream")?;
            send(&mut control_tx, &ClientMessage::LoadComplete { stream }).await?;
        }

        let _ = upload_semaphore
            .acquire_many(loading_streams)
            .await
            .unwrap();

        state_tx.send((TestState::Grace3, Instant::now())).unwrap();
        time::sleep(grace).await;
    }

    if let Some((semaphore, _)) = both_download.as_ref() {
        send(
            &mut control_tx,
            &ClientMessage::ScheduleLoads {
                groups: vec![1, 3],
                delay: load_delay,
            },
        )
        .await?;
        let load = scheduled_load_rx.recv().await.unwrap();
        state_tx.send((TestState::LoadFromBoth, load.time)).unwrap();
        msg(&format!("Testing both download and upload..."));

        for _ in 0..config.streams {
            let stream = upload_done_rx.recv().await.ok_or("Expected stream")?;
            send(&mut control_tx, &ClientMessage::LoadComplete { stream }).await?;
        }

        let _ = semaphore.acquire_many(loading_streams).await.unwrap();
        let _ = both_upload_semaphore
            .acquire_many(loading_streams)
            .await
            .unwrap();

        state_tx.send((TestState::Grace4, Instant::now())).unwrap();
        time::sleep(grace).await;
    }

    state_tx.send((TestState::End, Instant::now())).unwrap();

    if let Some(peer) = peer.as_mut() {
        peer.stop().await?;
    }

    // Wait for pings to return
    time::sleep(Duration::from_millis(500)).await;
    state_tx
        .send((TestState::EndPingRecv, Instant::now()))
        .unwrap();

    let peer = if let Some(peer) = peer {
        Some(peer.complete().await?)
    } else {
        None
    };

    let duration = start.elapsed();

    let pings_sent = ping_send.await?;
    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;
    send(&mut control_tx, &ClientMessage::Done).await?;

    let mut pongs = ping_recv.await?;

    let (mut latencies, bandwidth, server_overload) = measures.await?;

    let server_overload = server_overload || peer.as_ref().map(|p| p.0).unwrap_or_default();

    let peer_latencies = peer.map(|(_, latencies)| {
        latencies
            .into_iter()
            .enumerate()
            .map(|(i, p)| RawPing {
                index: i as u64,
                sent: Duration::from_micros(p.sent.wrapping_add(server_time_offset)),
                latency: p.latency,
            })
            .collect::<Vec<_>>()
    });

    let download_bytes = wait_on_download_loaders(download).await;
    let both_download_bytes = wait_on_download_loaders(both_download).await;

    latencies.sort_by_key(|d| d.index);
    pongs.sort_by_key(|d| d.0.index);
    let pings: Vec<_> = pings_sent
        .into_iter()
        .enumerate()
        .map(|(index, sent)| {
            let index = index as u64 + ping_start_index;
            let mut latency = latencies
                .binary_search_by_key(&index, |e| e.index)
                .ok()
                .map(|ping| RawLatency {
                    total: None,
                    up: Duration::from_micros(
                        latencies[ping].time.wrapping_add(server_time_offset),
                    )
                    .saturating_sub(sent),
                });

            latency.as_mut().map(|latency| {
                pongs
                    .binary_search_by_key(&index, |e| e.0.index)
                    .ok()
                    .map(|ping| {
                        latency.total = Some(pongs[ping].1.saturating_sub(sent));
                    });
            });

            RawPing {
                index,
                sent,
                latency,
            }
        })
        .collect();

    let mut raw_streams = Vec::new();

    let to_raw = |data: &[(u64, u64)]| -> RawStream {
        RawStream {
            data: data
                .iter()
                .map(|&(time, bytes)| RawPoint {
                    time: Duration::from_micros(time),
                    bytes,
                })
                .collect(),
        }
    };

    let mut add_down = |both, data: &Option<Vec<Vec<(u64, u64)>>>| {
        data.as_ref().map(|download_bytes| {
            raw_streams.push(RawStreamGroup {
                download: true,
                both,
                streams: download_bytes.iter().map(|stream| to_raw(stream)).collect(),
            });
        });
    };

    add_down(false, &download_bytes);
    add_down(true, &both_download_bytes);

    let get_stream = |group, id| -> Vec<_> {
        bandwidth
            .iter()
            .filter(|e| e.0.group == group && e.0.id == id)
            .map(|e| (e.1.wrapping_add(server_time_offset), e.2))
            .collect()
    };

    let get_raw_upload_bytes = |group| -> Vec<RawStream> {
        (0..loading_streams)
            .map(|i| to_raw(&get_stream(group, i)))
            .collect()
    };

    config.upload.then(|| {
        raw_streams.push(RawStreamGroup {
            download: false,
            both: false,
            streams: get_raw_upload_bytes(0),
        })
    });

    config.both.then(|| {
        raw_streams.push(RawStreamGroup {
            download: false,
            both: true,
            streams: get_raw_upload_bytes(1),
        })
    });

    let raw_config = RawConfig {
        stagger: config.stream_stagger,
        load_duration: config.load_duration,
        grace_duration: config.grace_duration,
        ping_interval: config.ping_interval,
        bandwidth_interval: config.bandwidth_interval,
    };

    if server_overload {
        msg(&format!(
            "Warning: Server overload detected during test. Result should be discarded."
        ));
    }

    let load_termination_timeout = state.timeout.load(Ordering::SeqCst);

    if load_termination_timeout {
        msg(&format!(
            "Warning: Load termination timed out. There may be residual untracked traffic in the background."
        ));
    }

    let start = start.duration_since(setup_start);

    let raw_result = RawResult {
        version: RawHeader::default().version,
        generated_by: format!("Crusader {}", env!("CARGO_PKG_VERSION")),
        config: raw_config,
        ipv6: server.is_ipv6(),
        load_termination_timeout,
        server_overload,
        server_latency: latency,
        start,
        duration,
        stream_groups: raw_streams,
        pings,
        peer_pings: peer_latencies,
    };

    Ok(raw_result)
}

pub(crate) async fn measure_latency(
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

pub fn save_raw(result: &RawResult, name: &str) -> String {
    let name = unique(name, "crr");
    result.save(Path::new(&name));
    name
}

fn setup_loaders(
    id: u64,
    server: SocketAddr,
    count: u64,
) -> Vec<JoinHandle<Framed<TcpStream, LengthDelimitedCodec>>> {
    (0..count)
        .map(|_| {
            tokio::spawn(async move {
                let stream = TcpStream::connect(server)
                    .await
                    .expect("unable to bind TCP socket");
                stream.set_nodelay(true).unwrap();
                let mut stream = Framed::new(stream, codec());
                hello_combined(&mut stream).await.unwrap();
                send(&mut stream, &ClientMessage::Associate(id))
                    .await
                    .unwrap();

                stream
            })
        })
        .collect()
}

fn upload_loaders(
    all_loaders: Arc<Semaphore>,
    id: u64,
    server: SocketAddr,
    group: u32,
    config: Config,
    stagger_offset: Duration,
    data: Arc<Vec<u8>>,
    state_rx: watch::Receiver<(TestState, Instant)>,
    state: TestState,
    done: Sender<TestStream>,
) {
    let loaders = setup_loaders(id, server, config.streams);

    for (i, loader) in loaders.into_iter().enumerate() {
        let mut state_rx = state_rx.clone();
        let data = data.clone();
        let all_loaders = all_loaders.clone();
        let done = done.clone();
        tokio::spawn(async move {
            let mut stream = loader.await.unwrap();

            let delay = config.stream_stagger * i as u32 + stagger_offset;

            let test_stream = TestStream {
                group,
                id: i as u32,
            };

            send(
                &mut stream,
                &ClientMessage::LoadFromClient {
                    stream: test_stream,
                    delay: delay.as_micros() as u64,
                    duration: (config.load_duration + MEASURE_DELAY).as_micros() as u64,
                    bandwidth_interval: config.bandwidth_interval.as_micros() as u64,
                },
            )
            .await
            .unwrap();
            let reply: ServerMessage = receive(&mut stream).await.unwrap();
            match reply {
                ServerMessage::WaitingForLoad => (),
                _ => panic!("Unexpected message {:?}", reply),
            };

            send(&mut stream, &ClientMessage::SendByte).await.unwrap();

            // Wait for a pending read byte
            {
                let mut stream_rx = stream.get_mut().split().0;
                loop {
                    let _ = stream_rx.read(&mut []).await.unwrap();
                    match time::timeout(Duration::from_millis(10), stream_rx.peek(&mut [0])).await {
                        Ok(Ok(1)) => break,
                        Err(_) | Ok(Ok(_)) => (),
                        Ok(Err(err)) => panic!("{:?}", err),
                    }
                }
            }

            all_loaders.add_permits(1);

            let start = wait_for_state(&mut state_rx, state).await + MEASURE_DELAY + delay;

            time::sleep_until(start).await;

            write_data(
                stream.into_inner(),
                data.as_ref(),
                start + config.load_duration,
            )
            .await
            .unwrap();

            done.send(test_stream).await.unwrap();
        });
    }
}

async fn wait_on_download_loaders(
    download: Option<(Arc<Semaphore>, Vec<JoinHandle<Vec<(u64, u64)>>>)>,
) -> Option<Vec<Vec<(u64, u64)>>> {
    match download {
        Some((_, result)) => {
            let bytes: Vec<_> = stream::iter(result)
                .then(|data| async move { data.await.unwrap() })
                .collect()
                .await;
            Some(bytes)
        }
        None => None,
    }
}

fn download_loaders(
    state: Arc<State>,
    all_loaders: Arc<Semaphore>,
    id: u64,
    server: SocketAddr,
    group: u32,
    config: Config,
    setup_start: Instant,
    state_rx: watch::Receiver<(TestState, Instant)>,
    test_state: TestState,
) -> (Arc<Semaphore>, Vec<JoinHandle<Vec<(u64, u64)>>>) {
    let semaphore = Arc::new(Semaphore::new(0));
    let loaders = setup_loaders(id, server, config.streams);

    let loaders = loaders
        .into_iter()
        .enumerate()
        .map(|(i, loader)| {
            let mut state_rx = state_rx.clone();
            let state = state.clone();
            let semaphore = semaphore.clone();
            let all_loaders = all_loaders.clone();

            tokio::spawn(async move {
                let mut stream = loader.await.unwrap();

                let mut buffer = Vec::with_capacity(512 * 1024);
                buffer.extend((0..buffer.capacity()).map(|_| 0));

                let delay = config.stream_stagger * i as u32;

                let test_stream = TestStream {
                    group,
                    id: i as u32,
                };

                send(
                    &mut stream,
                    &ClientMessage::LoadFromServer {
                        stream: test_stream,
                        duration: config.load_duration.as_micros() as u64,
                        delay: (MEASURE_DELAY + delay).as_micros() as u64,
                    },
                )
                .await
                .unwrap();

                let reply: ServerMessage = receive(&mut stream).await.unwrap();
                match reply {
                    ServerMessage::WaitingForByte => (),
                    _ => panic!("Unexpected message {:?}", reply),
                };

                stream.get_mut().write_u8(1).await.unwrap();

                let reply: ServerMessage = receive(&mut stream).await.unwrap();
                match reply {
                    ServerMessage::WaitingForLoad => (),
                    _ => panic!("Unexpected message {:?}", reply),
                };

                let stream = stream.into_inner();

                let (reading_done_tx, reading_done_rx) = oneshot::channel();

                state.downloads.lock().insert(test_stream, reading_done_tx);

                let bytes = Arc::new(AtomicU64::new(0));
                let bytes_ = bytes.clone();

                let done = Arc::new(AtomicBool::new(false));
                let done_ = done.clone();

                all_loaders.add_permits(1);

                let start = wait_for_state(&mut state_rx, test_state).await + delay;

                time::sleep_until(start).await;

                let measures = tokio::spawn(async move {
                    let mut measures = Vec::new();
                    let mut interval = time::interval(config.bandwidth_interval);
                    loop {
                        interval.tick().await;

                        let current_time = Instant::now();
                        let current_bytes = bytes_.load(Ordering::Acquire);

                        measures.push((
                            current_time.duration_since(setup_start).as_micros() as u64,
                            current_bytes,
                        ));

                        if done_.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    measures
                });

                let timeout = read_data(
                    stream,
                    &mut buffer,
                    bytes,
                    start + MEASURE_DELAY + config.load_duration,
                    reading_done_rx,
                )
                .await
                .unwrap();

                if timeout {
                    state.timeout.store(true, Ordering::SeqCst);
                }

                done.store(true, Ordering::Release);

                semaphore.add_permits(1);

                measures.await.unwrap()
            })
        })
        .collect();
    (semaphore, loaders)
}

async fn wait_for_state(
    state_rx: &mut watch::Receiver<(TestState, Instant)>,
    state: TestState,
) -> Instant {
    loop {
        {
            let current = state_rx.borrow_and_update();
            if current.0 == state {
                return current.1;
            }
        }
        state_rx.changed().await.unwrap();
    }
}

pub(crate) fn udp_handle(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            if e.raw_os_error() == Some(libc::ENOBUFS) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

pub(crate) async fn ping_send(
    mut ping_index: u64,
    id: u64,
    state_rx: watch::Receiver<(TestState, Instant)>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
    estimated_duration: Duration,
) -> Vec<Duration> {
    let mut storage = Vec::with_capacity(
        ((estimated_duration.as_secs_f64() + 2.0) * (1000.0 / interval.as_millis() as f64) * 1.5)
            as usize,
    );
    let mut buf = [0; 64];

    let mut interval = time::interval(interval);

    loop {
        interval.tick().await;

        if state_rx.borrow().0 >= TestState::End {
            break;
        }

        let current = setup_start.elapsed();

        let ping = Ping {
            id,
            index: ping_index,
        };

        ping_index += 1;

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping).unwrap();
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        udp_handle(socket.send(buf).await.map(|_| ())).expect("unable to udp ping");

        storage.push(current);
    }

    storage
}

pub(crate) async fn ping_recv(
    mut state_rx: watch::Receiver<(TestState, Instant)>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
    estimated_duration: Duration,
) -> Vec<(Ping, Duration)> {
    let mut storage = Vec::with_capacity(
        ((estimated_duration.as_secs_f64() + 2.0) * (1000.0 / interval.as_millis() as f64) * 1.5)
            as usize,
    );
    let mut buf = [0; 64];

    let end = wait_for_state(&mut state_rx, TestState::EndPingRecv).fuse();
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

pub fn timed(name: &str) -> String {
    let time = chrono::Local::now().format(" %Y.%m.%d %H-%M-%S");
    format!("{}{}", name, time)
}

pub(crate) fn unique(name: &str, ext: &str) -> String {
    let stem = timed(name);
    let mut i: usize = 0;
    loop {
        let file = if i != 0 {
            format!("{} {}", stem, i)
        } else {
            stem.to_string()
        };
        let file = format!("{}.{}", file, ext);
        if !Path::new(&file).exists() {
            return file;
        }
        i += 1;
    }
}

pub fn test(config: Config, plot: PlotConfig, host: &str, latency_peer_server: Option<&str>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(test_async(
            config,
            host,
            latency_peer_server,
            Arc::new(|msg| println!("{msg}")),
        ))
        .unwrap();
    println!("Writing data...");
    let raw = save_raw(&result, "data");
    println!("Saved raw data as {}", raw);
    let file = save_graph(&plot, &result.to_test_result(), "plot");
    println!("Saved plot as {}", file);
}

pub fn test_callback(
    config: Config,
    host: &str,
    latency_peer_server: Option<&str>,
    msg: Arc<dyn Fn(&str) + Send + Sync>,
    done: Box<dyn FnOnce(Option<Result<RawResult, String>>) + Send>,
) -> oneshot::Sender<()> {
    let (tx, rx) = oneshot::channel();
    let host = host.to_string();
    let latency_peer_server = latency_peer_server.map(|host| host.to_string());
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();

        done(rt.block_on(async move {
            let mut result = task::spawn(async move {
                test_async(config, &host, latency_peer_server.as_deref(), msg)
                    .await
                    .map_err(|error| error.to_string())
            })
            .fuse();

            select! {
                result = result => {
                    Some(result.map_err(|error| error.to_string()).and_then(|result| result))
                },
                result = rx.fuse() => {
                    result.unwrap();
                    None
                },
            }
        }));
    });
    tx
}
