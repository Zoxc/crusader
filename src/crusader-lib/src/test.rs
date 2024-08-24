use crate::common::{
    data, hello, measure_latency, ping_recv, ping_send, read_data, wait_for_state, write_data,
    Config, Msg, TestState,
};
use crate::file_format::{
    RawConfig, RawHeader, RawPing, RawPoint, RawResult, RawStream, RawStreamGroup,
};
use crate::peer::connect_to_peer;
use crate::plot::save_graph;
use crate::protocol::{
    codec, receive, send, ClientMessage, Hello, RawLatency, ServerMessage, TestStream,
};
use crate::{discovery, version, with_time};
use anyhow::{anyhow, bail, Context};
use bytes::{Bytes, BytesMut};
use futures::future::FutureExt;
use futures::{select, Sink, Stream};
use futures::{stream, StreamExt};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::{
    error::Error,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::{oneshot, watch, Semaphore};
use tokio::task::{self, JoinHandle};
use tokio::time::Instant;
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

const MEASURE_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug)]
struct ScheduledLoads {
    time: Instant,
}

struct State {
    downloads: Mutex<HashMap<TestStream, oneshot::Sender<()>>>,
    timeout: AtomicBool,
}

async fn hello_combined<S: Sink<Bytes> + Stream<Item = Result<BytesMut, S::Error>> + Unpin>(
    stream: &mut S,
) -> Result<(), anyhow::Error>
where
    S::Error: Error + Send + Sync + 'static,
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

#[derive(Default)]
pub struct PlotConfig {
    pub split_throughput: bool,
    pub transferred: bool,
    pub max_throughput: Option<u64>,
    pub max_latency: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub title: Option<String>,
}

pub(crate) async fn test_async(
    config: Config,
    server: Option<&str>,
    latency_peer_server: Option<&str>,
    msg: Msg,
) -> Result<RawResult, anyhow::Error> {
    msg(&format!("Client version {} running", version()));

    let control = if let Some(server) = server {
        net::TcpStream::connect((server, config.port))
            .await
            .context("Failed to connect to server")?
    } else {
        let server = discovery::locate().await?;
        msg(&format!(
            "Found server at {} running version {}",
            server.at, server.software_version
        ));
        net::TcpStream::connect(server.socket)
            .await
            .context("Failed to connect to server")?
    };

    control.set_nodelay(true)?;

    let server = control.peer_addr()?;

    msg(&format!("Connected to server {}", server));

    let (rx, tx) = control.into_split();
    let mut control_rx = FramedRead::new(rx, codec());
    let mut control_tx = FramedWrite::new(tx, codec());

    hello(&mut control_tx, &mut control_rx)
        .await
        .context("Failed protocol handshake")?;

    send(&mut control_tx, &ClientMessage::NewClient).await?;

    let setup_start = Instant::now();

    let reply: ServerMessage = receive(&mut control_rx)
        .await
        .context("Failed to create a new client id")?;
    let id = match reply {
        ServerMessage::NewClient(Some(id)) => id,
        ServerMessage::NewClient(None) => bail!("Server was unable to create client"),
        _ => bail!("Unexpected message {:?}", reply),
    };

    let loading_streams: u32 = config.streams.try_into()?;

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
        "Idle latency to server {:.2} ms",
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
        let mut throughput = Vec::new();
        let mut latencies = Vec::new();
        let overload_;

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await?;
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
                    throughput.push((stream, time, bytes));
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
                        .ok_or(anyhow!("Failed to find stream"))?
                        .send(())
                        .map_err(|_| anyhow!("Failed to notify downloader"))?;
                }
                ServerMessage::ScheduledLoads { groups: _, time } => {
                    let time = Duration::from_micros(time.wrapping_add(server_time_offset));
                    scheduled_load_tx
                        .send(ScheduledLoads {
                            time: setup_start + time,
                        })
                        .await?
                }
                _ => bail!("Unexpected message {:?}", reply),
            };
        }

        Ok((latencies, throughput, overload_))
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

    state_tx.send((TestState::Grace1, start))?;
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
        let load = scheduled_load_rx
            .recv()
            .await
            .ok_or(anyhow!("Failed to receive"))?;
        state_tx.send((TestState::LoadFromServer, load.time))?;
        msg(&format!("Testing download..."));
        let _ = semaphore.acquire_many(loading_streams).await?;

        state_tx.send((TestState::Grace2, Instant::now()))?;
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
        let load = scheduled_load_rx
            .recv()
            .await
            .ok_or(anyhow!("Failed to receive"))?;
        state_tx.send((TestState::LoadFromClient, load.time))?;
        msg(&format!("Testing upload..."));

        for _ in 0..config.streams {
            let stream = upload_done_rx
                .recv()
                .await
                .ok_or(anyhow!("Expected stream"))?;
            send(&mut control_tx, &ClientMessage::LoadComplete { stream }).await?;
        }

        let _ = upload_semaphore.acquire_many(loading_streams).await?;

        state_tx.send((TestState::Grace3, Instant::now()))?;
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
        let load = scheduled_load_rx
            .recv()
            .await
            .ok_or(anyhow!("Failed to receive"))?;
        state_tx.send((TestState::LoadFromBoth, load.time))?;
        msg(&format!("Testing both download and upload..."));

        for _ in 0..config.streams {
            let stream = upload_done_rx
                .recv()
                .await
                .ok_or(anyhow!("Expected stream"))?;
            send(&mut control_tx, &ClientMessage::LoadComplete { stream }).await?;
        }

        let _ = semaphore.acquire_many(loading_streams).await?;
        let _ = both_upload_semaphore.acquire_many(loading_streams).await?;

        state_tx.send((TestState::Grace4, Instant::now()))?;
        time::sleep(grace).await;
    }

    state_tx.send((TestState::End, Instant::now()))?;

    if let Some(peer) = peer.as_mut() {
        peer.stop().await?;
    }

    // Wait for pings to return
    time::sleep(Duration::from_millis(500)).await;
    state_tx.send((TestState::EndPingRecv, Instant::now()))?;

    let peer = if let Some(peer) = peer {
        Some(peer.complete().await?)
    } else {
        None
    };

    let duration = start.elapsed();

    let pings_sent = ping_send.await??;
    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;
    send(&mut control_tx, &ClientMessage::Done).await?;

    let mut pongs = ping_recv.await??;

    let (mut latencies, throughput, server_overload) = measures.await??;

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

    let download_bytes = wait_on_download_loaders(download).await?;
    let both_download_bytes = wait_on_download_loaders(both_download).await?;

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
        throughput
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
        bandwidth_interval: config.throughput_interval,
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
        generated_by: format!("Crusader {}", version()),
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

pub fn save_raw(result: &RawResult, name: &str, root_path: &Path) -> Result<String, anyhow::Error> {
    std::fs::create_dir_all(root_path)?;
    let name = unique(name, "crr");
    result.save(&root_path.join(&name))?;
    Ok(name)
}

fn setup_loaders(
    id: u64,
    server: SocketAddr,
    count: u64,
) -> Vec<JoinHandle<Result<Framed<TcpStream, LengthDelimitedCodec>, anyhow::Error>>> {
    (0..count)
        .map(|_| {
            tokio::spawn(async move {
                let stream = TcpStream::connect(server)
                    .await
                    .context("unable to bind TCP socket")?;
                stream.set_nodelay(true)?;
                let mut stream = Framed::new(stream, codec());
                hello_combined(&mut stream).await?;
                send(&mut stream, &ClientMessage::Associate(id)).await?;

                Ok(stream)
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
            let mut stream = loader.await??;

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
                    throughput_interval: config.throughput_interval.as_micros() as u64,
                },
            )
            .await?;
            let reply: ServerMessage = receive(&mut stream).await?;
            match reply {
                ServerMessage::WaitingForLoad => (),
                _ => panic!("Unexpected message {:?}", reply),
            };

            send(&mut stream, &ClientMessage::SendByte).await?;

            // Wait for a pending read byte
            {
                let mut stream_rx = stream.get_mut().split().0;
                loop {
                    let _ = stream_rx.read(&mut []).await?;
                    match time::timeout(Duration::from_millis(10), stream_rx.peek(&mut [0])).await {
                        Ok(Ok(1)) => break,
                        Err(_) | Ok(Ok(_)) => (),
                        Ok(Err(err)) => panic!("{:?}", err),
                    }
                }
            }

            all_loaders.add_permits(1);

            let start = wait_for_state(&mut state_rx, state).await? + MEASURE_DELAY + delay;

            time::sleep_until(start).await;

            write_data(
                stream.into_inner(),
                data.as_ref(),
                start + config.load_duration,
            )
            .await
            .unwrap();

            done.send(test_stream).await?;
            Ok::<(), anyhow::Error>(())
        });
    }
}

async fn wait_on_download_loaders(
    download: Option<(
        Arc<Semaphore>,
        Vec<JoinHandle<Result<Vec<(u64, u64)>, anyhow::Error>>>,
    )>,
) -> Result<Option<Vec<Vec<(u64, u64)>>>, anyhow::Error> {
    match download {
        Some((_, result)) => {
            let bytes: Vec<_> = stream::iter(result)
                .then(|data| async move { data.await? })
                .collect()
                .await;
            let bytes: Result<Vec<_>, _> = bytes.into_iter().collect();
            Ok(Some(bytes?))
        }
        None => Ok(None),
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
) -> (
    Arc<Semaphore>,
    Vec<JoinHandle<Result<Vec<(u64, u64)>, anyhow::Error>>>,
) {
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
                let mut stream = loader.await??;

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
                .await?;

                let reply: ServerMessage = receive(&mut stream).await?;
                match reply {
                    ServerMessage::WaitingForByte => (),
                    _ => panic!("Unexpected message {:?}", reply),
                };

                stream.get_mut().write_u8(1).await?;

                let reply: ServerMessage = receive(&mut stream).await?;
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

                let start = wait_for_state(&mut state_rx, test_state).await? + delay;

                time::sleep_until(start).await;

                let measures = tokio::spawn(async move {
                    let mut measures = Vec::new();
                    let mut interval = time::interval(config.throughput_interval);
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
                .await?;

                if timeout {
                    state.timeout.store(true, Ordering::SeqCst);
                }

                done.store(true, Ordering::Release);

                semaphore.add_permits(1);

                Ok::<_, anyhow::Error>(measures.await?)
            })
        })
        .collect();
    (semaphore, loaders)
}

pub fn timed(name: &str) -> String {
    let time = chrono::Local::now().format(" %Y-%m-%d %H.%M.%S");
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

pub fn test(
    config: Config,
    plot: PlotConfig,
    host: Option<&str>,
    latency_peer_server: Option<&str>,
) -> Result<(), anyhow::Error> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(test_async(
        config,
        host,
        latency_peer_server,
        Arc::new(|msg| println!("{}", with_time(msg))),
    ));
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            println!("{}", with_time(&format!("Client failed")));
            return Err(error);
        }
    };
    println!("{}", with_time("Writing data..."));
    let path = Path::new("crusader-results");
    let raw = save_raw(&result, "data", path)?;
    println!(
        "{}",
        with_time(&format!("Saved raw data as {}", path.join(raw).display()))
    );
    let plot = save_graph(&plot, &result.to_test_result(), "plot", path)?;
    println!(
        "{}",
        with_time(&format!("Saved plot as {}", path.join(plot).display()))
    );
    Ok(())
}

pub fn test_callback(
    config: Config,
    host: Option<&str>,
    latency_peer_server: Option<&str>,
    msg: Arc<dyn Fn(&str) + Send + Sync>,
    done: Box<dyn FnOnce(Option<Result<RawResult, String>>) + Send>,
) -> oneshot::Sender<()> {
    let (tx, rx) = oneshot::channel();
    let host = host.map(|host| host.to_string());
    let latency_peer_server = latency_peer_server.map(|host| host.to_string());
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();

        done(rt.block_on(async move {
            let mut result = task::spawn(async move {
                test_async(config, host.as_deref(), latency_peer_server.as_deref(), msg)
                    .await
                    .map_err(|error| format!("{:?}", error))
            })
            .fuse();

            select! {
                result = result => {
                    Some(result.map_err(|error| error.to_string()).and_then(|result| result))
                },
                result = rx.fuse() => {
                    result.ok();
                    None
                },
            }
        }));
    });
    tx
}
