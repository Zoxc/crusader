use bytes::{Bytes, BytesMut};
use futures::future::FutureExt;
use futures::{pin_mut, select, Sink, Stream};
use futures::{stream, StreamExt};
use rand::prelude::StdRng;
use rand::Rng;
use rand::SeedableRng;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    error::Error,
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{watch, Semaphore};
use tokio::task::{self, yield_now, JoinHandle};
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{Framed, FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::protocol::{
    codec, receive, send, ClientMessage, Hello, Ping, ServerMessage, TestStream,
};
use crate::serve2::CountingCodec;

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum TestState {
    Setup,
    Grace1,
    LoadFromClient,
    Grace2,
    LoadFromServer,
    Grace3,
    LoadFromBoth,
    Grace4,
    End,
}

pub(crate) fn data() -> Vec<u8> {
    let mut vec = Vec::with_capacity(512 * 1024);
    let mut rng = StdRng::from_seed([
        18, 141, 186, 158, 195, 76, 244, 56, 219, 131, 65, 128, 250, 63, 228, 44, 233, 34, 9, 51,
        13, 72, 230, 131, 223, 240, 124, 77, 103, 238, 103, 186,
    ]);
    for _ in 0..vec.capacity() {
        vec.push(rng.gen())
    }
    vec
}

async fn hello<S: Sink<Bytes> + Stream<Item = Result<BytesMut, S::Error>> + Unpin>(
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

async fn test_async(server: &str) -> Result<(), Box<dyn Error>> {
    let port = 30481;

    let control = net::TcpStream::connect((server, port)).await?;

    let server = control.peer_addr()?;

    println!("Connected to server {}", server);

    let mut control = Framed::new(control, codec());

    hello(&mut control).await?;

    let bandwidth_interval = Duration::from_millis(10);

    send(&mut control, &ClientMessage::NewClient).await?;

    let setup_start = Instant::now();

    let reply: ServerMessage = receive(&mut control).await?;
    let id = match reply {
        ServerMessage::NewClient(id) => id,
        _ => panic!("Unexpected message {:?}", reply),
    };

    let local_udp = if server.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let data = Arc::new(data());

    let loading_streams: u32 = 16;

    let grace = Duration::from_secs(1);
    let load_duration = Duration::from_secs(5);
    let ping_interval = Duration::from_millis(10);
    let estimated_duration = load_duration * 1 + grace * 2;

    let (state_tx, state_rx) = watch::channel(TestState::Setup);

    upload_loaders(
        id,
        server,
        0,
        loading_streams,
        data.clone(),
        bandwidth_interval,
        state_rx.clone(),
        TestState::LoadFromClient,
    );

    upload_loaders(
        id,
        server,
        1,
        loading_streams,
        data.clone(),
        bandwidth_interval,
        state_rx.clone(),
        TestState::LoadFromBoth,
    );

    let (download_semaphore, download) = download_loaders(
        id,
        server,
        loading_streams,
        bandwidth_interval,
        setup_start,
        state_rx.clone(),
        TestState::LoadFromServer,
    );

    let (both_download_semaphore, both_download) = download_loaders(
        id,
        server,
        loading_streams,
        bandwidth_interval,
        setup_start,
        state_rx.clone(),
        TestState::LoadFromBoth,
    );

    send(&mut control, &ClientMessage::GetMeasurements).await?;

    let (rx, tx) = control.into_inner().into_split();
    let mut rx = FramedRead::new(rx, codec());
    let mut tx = FramedWrite::new(tx, codec());

    let upload_semaphore = Arc::new(Semaphore::new(0));
    let upload_semaphore_ = upload_semaphore.clone();
    let both_upload_semaphore = Arc::new(Semaphore::new(0));
    let both_upload_semaphore_ = both_upload_semaphore.clone();

    let bandwidth = tokio::spawn(async move {
        let mut bandwidth = Vec::new();

        loop {
            let reply: ServerMessage = receive(&mut rx).await.unwrap();
            match reply {
                ServerMessage::MeasureStreamDone { stream } => {
                    if stream.group == 0 {
                        &upload_semaphore_
                    } else {
                        &both_upload_semaphore_
                    }
                    .add_permits(1);
                }
                ServerMessage::Measure {
                    stream,
                    time,
                    bytes,
                } => {
                    //let mbits = (bytes as f64 * 8.0) / 1000.0 / 1000.0;
                    //let rate = mbits / Duration::from_micros(duration).as_secs_f64();
                    //println!("Rate: {:>10.2} Mbps, Bytes: {}", rate, bytes);
                    bandwidth.push((stream, time, bytes));
                }
                ServerMessage::MeasurementsDone => break,
                _ => panic!("Unexpected message {:?}", reply),
            };
        }

        bandwidth
    });

    let ping_send = tokio::spawn(ping_send(
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
    ));

    let ping_recv = tokio::spawn(ping_recv(
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
        estimated_duration,
    ));

    time::sleep(Duration::from_millis(100)).await;

    let start = Instant::now();

    state_tx.send(TestState::Grace1).unwrap();
    time::sleep(grace).await;

    state_tx.send(TestState::LoadFromServer).unwrap();
    task::spawn_blocking(|| println!("Testing download..."));
    time::sleep(load_duration).await;

    state_tx.send(TestState::Grace2).unwrap();
    let _ = download_semaphore
        .acquire_many(loading_streams)
        .await
        .unwrap();
    time::sleep(grace).await;

    state_tx.send(TestState::LoadFromClient).unwrap();
    task::spawn_blocking(|| println!("Testing upload..."));
    time::sleep(load_duration).await;

    state_tx.send(TestState::Grace3).unwrap();
    let _ = upload_semaphore
        .acquire_many(loading_streams)
        .await
        .unwrap();
    time::sleep(grace).await;

    state_tx.send(TestState::LoadFromBoth).unwrap();
    task::spawn_blocking(|| println!("Testing both download and upload..."));
    time::sleep(load_duration).await;

    state_tx.send(TestState::Grace4).unwrap();
    let _ = both_download_semaphore
        .acquire_many(loading_streams)
        .await
        .unwrap();
    let _ = both_upload_semaphore
        .acquire_many(loading_streams)
        .await
        .unwrap();
    time::sleep(grace).await;

    state_tx.send(TestState::End).unwrap();

    let duration = start.elapsed();

    ping_send.await?;
    let mut pings = ping_recv.await?;

    pings.sort_by_key(|d| d.0.index);

    send(&mut tx, &ClientMessage::Done).await?;

    println!("Test duration {:?}", duration);

    let bandwidth = bandwidth.await?;

    let download_bytes: Vec<_> = stream::iter(download)
        .then(|data| async move { data.await.unwrap() })
        .collect()
        .await;

    let both_download_bytes: Vec<_> = stream::iter(both_download)
        .then(|data| async move { data.await.unwrap() })
        .collect()
        .await;

    println!("Writing graphs...");

    let download_bytes: Vec<_> = download_bytes
        .iter()
        .map(|stream| to_float(&stream))
        .collect();

    let both_download_bytes: Vec<_> = both_download_bytes
        .iter()
        .map(|stream| to_float(&stream))
        .collect();

    let download_bytes_sum = sum_bytes(download_bytes, bandwidth_interval);

    let both_download_bytes_sum = sum_bytes(both_download_bytes, bandwidth_interval);

    let combined_download_bytes = sum_bytes(
        vec![download_bytes_sum.clone(), both_download_bytes_sum.clone()],
        bandwidth_interval,
    );

    let get_stream = |group, id| {
        bandwidth
            .iter()
            .filter(|e| e.0.group == group && e.0.id == id)
            .map(|e| (e.1, e.2))
            .collect()
    };

    let get_upload_bytes = |group| -> Vec<Vec<_>> {
        let streams: Vec<Vec<_>> = (0..loading_streams).map(|i| get_stream(group, i)).collect();

        streams
            .into_iter()
            .map(|stream| to_float(&stream))
            .collect()
    };

    let upload_bytes_sum = sum_bytes(get_upload_bytes(0), bandwidth_interval);

    let both_upload_bytes_sum = sum_bytes(get_upload_bytes(1), bandwidth_interval);

    let combined_upload_bytes = sum_bytes(
        vec![upload_bytes_sum.clone(), both_upload_bytes_sum.clone()],
        bandwidth_interval,
    );

    let both_bytes = sum_bytes(
        vec![
            both_download_bytes_sum.clone(),
            both_upload_bytes_sum.clone(),
        ],
        bandwidth_interval,
    );

    graph(
        "plot.png",
        &pings,
        to_rates(&combined_upload_bytes),
        to_rates(&combined_download_bytes),
        to_rates(&both_bytes),
        &[&download_bytes_sum, &both_download_bytes_sum],
        &[&upload_bytes_sum, &both_upload_bytes_sum],
        &[&both_bytes],
        start.duration_since(setup_start).as_secs_f64(),
        duration.as_secs_f64(),
    );

    Ok(())
}

fn float_max(iter: impl Iterator<Item = f64>) -> f64 {
    let mut max = iter.fold(0. / 0., f64::max);

    if max.is_nan() {
        max = 100.0;
    }

    max
}

fn to_float(stream: &[(u64, u64)]) -> Vec<(u64, f64)> {
    stream.iter().map(|(t, v)| (*t, *v as f64)).collect()
}

fn to_rates(stream: &[(u64, f64)]) -> Vec<(u64, f64)> {
    (0..stream.len())
        .map(|i| {
            let rate = if i > 0 {
                let bytes = stream[i].1 - stream[i - 1].1;
                let duration = Duration::from_micros(stream[i].0 - stream[i - 1].0);
                let mbits = (bytes as f64 * 8.0) / (1000.0 * 1000.0);
                mbits / duration.as_secs_f64()
            } else {
                0.0
            };
            (stream[i].0, rate)
        })
        .collect()
}

fn sum_bytes(input: Vec<Vec<(u64, f64)>>, interval: Duration) -> Vec<(u64, f64)> {
    let interval = interval.as_micros() as u64;

    let bandwidth: Vec<_> = input
        .into_iter()
        .map(|stream| interpolate(stream, interval))
        .collect();

    let min = bandwidth
        .iter()
        .map(|stream| stream.first().map(|e| e.0).unwrap_or(0))
        .min()
        .unwrap_or(0);

    let max = bandwidth
        .iter()
        .map(|stream| stream.last().map(|e| e.0).unwrap_or(0))
        .max()
        .unwrap_or(0);

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let value = bandwidth
            .iter()
            .map(
                |stream| match stream.binary_search_by_key(&point, |e| e.0) {
                    Ok(i) => stream[i].1,
                    Err(0) => 0.0,
                    Err(i) if i == stream.len() => stream.last().unwrap().1,
                    _ => panic!("unexpected index"),
                },
            )
            .sum();
        data.push((point, value));
    }

    data
}

fn interpolate(input: Vec<(u64, f64)>, interval: u64) -> Vec<(u64, f64)> {
    if input.is_empty() {
        return Vec::new();
    }

    let min = input.first().unwrap().0 / interval * interval;
    let max = (input.last().unwrap().0 + interval - 1) / interval * interval;

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let i = input.partition_point(|e| e.0 < point);
        let value = if i == input.len() {
            input.last().unwrap().1
        } else if input[i].0 == point || i == 0 {
            input[i].1
        } else {
            let len = input[i].0 - input[i - 1].0;
            if len == 0 {
                input[i].1
            } else {
                let ratio = (point - input[i - 1].0) as f64 / len as f64;
                let delta = input[i].1 - input[i - 1].1;
                input[i - 1].1 + delta * ratio
            }
        };
        data.push((point, value));
    }

    data
}

fn setup_loaders(
    id: u64,
    server: SocketAddr,
    count: u32,
) -> Vec<JoinHandle<Framed<TcpStream, LengthDelimitedCodec>>> {
    (0..count)
        .map(|_| {
            tokio::spawn(async move {
                let stream = TcpStream::connect(server)
                    .await
                    .expect("unable to bind TCP socket");
                let mut stream = Framed::new(stream, codec());
                hello(&mut stream).await.unwrap();
                send(&mut stream, &ClientMessage::Associate(id))
                    .await
                    .unwrap();

                stream
            })
        })
        .collect()
}

fn upload_loaders(
    id: u64,
    server: SocketAddr,
    group: u32,
    count: u32,
    data: Arc<Vec<u8>>,
    bandwidth_interval: Duration,
    state_rx: watch::Receiver<TestState>,
    state: TestState,
) {
    let loaders = setup_loaders(id, server, count);

    for (i, loader) in loaders.into_iter().enumerate() {
        let mut state_rx = state_rx.clone();
        let data = data.clone();
        tokio::spawn(async move {
            let mut stream = loader.await.unwrap();

            send(
                &mut stream,
                &ClientMessage::LoadFromClient {
                    stream: TestStream {
                        group,
                        id: i as u32,
                    },
                    bandwidth_interval: bandwidth_interval.as_micros() as u64,
                },
            )
            .await
            .unwrap();

            let mut raw = stream.into_inner();

            wait_for_state(&mut state_rx, state).await;

            loop {
                raw.write(data.as_ref()).await.unwrap();

                if *state_rx.borrow() != state {
                    break;
                }

                yield_now().await;
            }
        });
    }
}

fn download_loaders(
    id: u64,
    server: SocketAddr,
    count: u32,
    bandwidth_interval: Duration,
    setup_start: Instant,
    state_rx: watch::Receiver<TestState>,
    state: TestState,
) -> (Arc<Semaphore>, Vec<JoinHandle<Vec<(u64, u64)>>>) {
    let semaphore = Arc::new(Semaphore::new(0));
    let loaders = setup_loaders(id, server, count);

    let loaders = loaders
        .into_iter()
        .map(|loader| {
            let mut state_rx = state_rx.clone();
            let semaphore = semaphore.clone();

            tokio::spawn(async move {
                let stream = loader.await.unwrap();

                let (rx, tx) = stream.into_inner().into_split();
                let mut tx = FramedWrite::new(tx, codec());
                let mut rx = FramedRead::with_capacity(rx, CountingCodec, 512 * 1024);

                wait_for_state(&mut state_rx, state).await;

                send(&mut tx, &ClientMessage::LoadFromServer).await.unwrap();

                tokio::spawn(async move {
                    loop {
                        if *state_rx.borrow_and_update() != state {
                            break;
                        }
                        state_rx.changed().await.unwrap();

                        send(&mut tx, &ClientMessage::Done).await.unwrap();
                    }
                });

                let bytes = Arc::new(AtomicU64::new(0));
                let bytes_ = bytes.clone();

                let done = Arc::new(AtomicBool::new(false));
                let done_ = done.clone();

                let measures = tokio::spawn(async move {
                    let mut measures = Vec::new();
                    let mut interval = time::interval(bandwidth_interval);
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

                while let Some(size) = rx.next().await {
                    let size = size.unwrap();
                    bytes.fetch_add(size as u64, Ordering::Release);
                    yield_now().await;
                }

                done.store(true, Ordering::Release);

                semaphore.add_permits(1);

                measures.await.unwrap()
            })
        })
        .collect();
    (semaphore, loaders)
}

async fn wait_for_state(state_rx: &mut watch::Receiver<TestState>, state: TestState) {
    loop {
        if *state_rx.borrow_and_update() == state {
            break;
        }
        state_rx.changed().await.unwrap();
    }
}

async fn ping_send(
    state_rx: watch::Receiver<TestState>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
) {
    let mut buf = [0; 64];
    let mut index: u32 = 0;

    let mut interval = time::interval(interval);

    loop {
        interval.tick().await;

        if *state_rx.borrow() == TestState::End {
            break;
        }

        let current = setup_start.elapsed();

        let ping = Ping {
            index,
            timestamp: current.as_micros() as u64,
        };

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping).unwrap();
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        socket.send(buf).await.expect("unable to udp ping");

        index += 1;
    }
}

async fn ping_recv(
    mut state_rx: watch::Receiver<TestState>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
    estimated_duration: Duration,
) -> Vec<(Ping, u64)> {
    let mut storage = Vec::with_capacity(
        ((estimated_duration.as_secs_f64() + 2.0) * (1000.0 / interval.as_millis() as f64) * 1.5)
            as usize,
    );
    let mut buf = [0; 64];

    let end = wait_for_state(&mut state_rx, TestState::End).fuse();
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

        let latency = (current.as_micros() as u64).saturating_sub(ping.timestamp);

        //println!("Ping {}", latency as f64 / 1000.0);

        storage.push((ping, latency));
    }

    storage
}

fn graph(
    path: &str,
    pings: &Vec<(Ping, u64)>,
    upload: Vec<(u64, f64)>,
    download: Vec<(u64, f64)>,
    both: Vec<(u64, f64)>,
    bytes_down: &[&Vec<(u64, f64)>],
    bytes_up: &[&Vec<(u64, f64)>],
    bytes_both: &[&Vec<(u64, f64)>],
    start: f64,
    duration: f64,
) {
    use plotters::prelude::*;

    let root = BitMapBackend::new(path, (1800, 1000)).into_drawing_area();

    root.fill(&WHITE).unwrap();

    let root = root
        .titled("Latency under load", (FontFamily::SansSerif, 26))
        .unwrap();

    let areas = root.split_evenly((3, 1));

    let max_latency = pings.iter().map(|d| d.1).max().unwrap_or(100) as f64 / 1000.0;

    let max_bytes = float_max(
        [bytes_down, bytes_up, bytes_both]
            .iter()
            .flat_map(|list| list.iter())
            .flat_map(|list| list.iter())
            .map(|e| e.1),
    );

    let max_bytes = max_bytes / (1000.0 * 1000.0 * 1000.0);

    let max_bandwidth = float_max(
        [&download, &upload, &both]
            .iter()
            .flat_map(|list| list.iter())
            .map(|e| e.1),
    );

    let max_bytes = max_bytes * 1.05;
    let max_bandwidth = max_bandwidth * 1.05;
    let max_latency = max_latency * 1.05;

    let font = (FontFamily::SansSerif, 18);

    // Scale to fit the legend
    let duration = duration * 1.08;

    let mut chart = ChartBuilder::on(&areas[1])
        .margin(6)
        .set_label_area_size(LabelAreaPosition::Left, 100)
        .set_label_area_size(LabelAreaPosition::Right, 100)
        .set_label_area_size(LabelAreaPosition::Bottom, 50)
        .build_cartesian_2d(0.0..duration, 0.0..max_latency)
        .unwrap();

    chart
        .plotting_area()
        .fill(&RGBColor(248, 248, 248))
        .unwrap();

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .x_labels(20)
        .y_labels(10)
        .x_label_style(font)
        .y_label_style(font)
        .y_desc("Latency (ms)")
        .x_desc("Elapsed time (seconds)")
        .draw()
        .unwrap();

    chart
        .draw_series(LineSeries::new(
            pings.iter().map(|(ping, latency)| {
                (
                    Duration::from_micros(ping.timestamp).as_secs_f64() - start,
                    *latency as f64 / 1000.0,
                )
            }),
            &RGBColor(50, 50, 50),
        ))
        .unwrap()
        .label("Latency")
        .legend(move |(x, y)| {
            Rectangle::new([(x, y - 5), (x + 18, y + 3)], RGBColor(50, 50, 50).filled())
        });

    let mut chart = ChartBuilder::on(&areas[0])
        .margin(6)
        .set_label_area_size(LabelAreaPosition::Left, 100)
        .set_label_area_size(LabelAreaPosition::Right, 100)
        .set_label_area_size(LabelAreaPosition::Bottom, 50)
        .build_cartesian_2d(0.0..duration, 0.0..max_bandwidth)
        .unwrap();

    chart
        .plotting_area()
        .fill(&RGBColor(248, 248, 248))
        .unwrap();

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .x_labels(20)
        .y_labels(10)
        .x_label_style(font)
        .y_label_style(font)
        .y_desc("Bandwidth (Mbps)")
        .draw()
        .unwrap();

    chart
        .draw_series(LineSeries::new(
            both.iter()
                .map(|(time, rate)| (Duration::from_micros(*time).as_secs_f64() - start, *rate)),
            &RGBColor(149, 96, 153),
        ))
        .unwrap()
        .label("Both")
        .legend(move |(x, y)| {
            Rectangle::new(
                [(x, y - 5), (x + 18, y + 3)],
                RGBColor(149, 96, 153).filled(),
            )
        });

    chart
        .draw_series(LineSeries::new(
            download
                .iter()
                .map(|(time, rate)| (Duration::from_micros(*time).as_secs_f64() - start, *rate)),
            &RGBColor(95, 145, 62),
        ))
        .unwrap()
        .label("Download")
        .legend(move |(x, y)| {
            Rectangle::new(
                [(x, y - 5), (x + 18, y + 3)],
                RGBColor(95, 145, 62).filled(),
            )
        });

    chart
        .draw_series(LineSeries::new(
            upload
                .iter()
                .map(|(time, rate)| (Duration::from_micros(*time).as_secs_f64() - start, *rate)),
            &RGBColor(37, 83, 169),
        ))
        .unwrap()
        .label("Upload")
        .legend(move |(x, y)| {
            Rectangle::new(
                [(x, y - 5), (x + 18, y + 3)],
                RGBColor(37, 83, 169).filled(),
            )
        });

    chart
        .configure_series_labels()
        .background_style(&WHITE.mix(0.8))
        .label_font(font)
        .border_style(&BLACK)
        .draw()
        .unwrap();

    let mut chart = ChartBuilder::on(&areas[2])
        .margin(6)
        .set_label_area_size(LabelAreaPosition::Left, 100)
        .set_label_area_size(LabelAreaPosition::Right, 100)
        .set_label_area_size(LabelAreaPosition::Bottom, 50)
        .build_cartesian_2d(0.0..duration, 0.0..max_bytes)
        .unwrap();

    chart
        .plotting_area()
        .fill(&RGBColor(248, 248, 248))
        .unwrap();

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .x_labels(20)
        .y_labels(10)
        .x_label_style(font)
        .y_label_style(font)
        .y_desc("Bytes transferred")
        .draw()
        .unwrap();

    let data = [
        ("Download", RGBColor(95, 145, 62), bytes_down),
        ("Upload", RGBColor(37, 83, 169), bytes_up),
        ("Both", RGBColor(149, 96, 153), bytes_both),
    ];

    for (name, color, bytes) in data {
        for (i, bytes) in bytes.iter().enumerate() {
            let series = chart
                .draw_series(LineSeries::new(
                    bytes.iter().map(|(time, bytes)| {
                        (
                            Duration::from_micros(*time).as_secs_f64() - start,
                            *bytes / (1000.0 * 1000.0 * 1000.0),
                        )
                    }),
                    &color,
                ))
                .unwrap();

            if i == 0 {
                series.label(name).legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 18, y + 3)], color.filled())
                });
            }
        }
    }

    chart
        .configure_series_labels()
        .background_style(&WHITE.mix(0.8))
        .label_font(font)
        .border_style(&BLACK)
        .draw()
        .unwrap();

    // To avoid the IO failure being ignored silently, we manually call the present function
    root.present().expect("Unable to write result to file, please make sure 'plotters-doc-data' dir exists under current dir");
}

pub fn test(host: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(test_async(host)).unwrap();
}
