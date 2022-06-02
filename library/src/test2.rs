use bytes::{Bytes, BytesMut};
use futures::future::FutureExt;
use futures::StreamExt;
use futures::{pin_mut, select, stream, Sink, Stream};
use rand::prelude::StdRng;
use rand::Rng;
use rand::SeedableRng;
use std::{
    error::Error,
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::yield_now;
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{Framed, FramedRead, FramedWrite};

use crate::protocol::{
    codec, receive, send, ClientMessage, Hello, Ping, ServerMessage, TestStream,
};

#[derive(PartialEq, Eq, Debug)]
enum TestState {
    Setup,
    Grace1,
    LoadFromClient,
    Grace2,
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

    send(
        &mut control,
        &ClientMessage::NewClient {
            bandwidth_interval: bandwidth_interval.as_micros() as u64,
        },
    )
    .await?;

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

    let loading_streams: u32 = 1;

    let grace = Duration::from_secs(2);
    let load_duration = Duration::from_secs(9);
    let ping_interval = Duration::from_millis(10);
    let estimated_duration = load_duration * 1 + grace * 2;

    let loaders: Vec<_> = stream::iter(0..loading_streams)
        .then(|stream_id| {
            let data = data.clone();
            async move {
                let stream = TcpStream::connect(server)
                    .await
                    .expect("unable to bind TCP socket");
                let mut stream = Framed::new(stream, codec());
                hello(&mut stream).await.unwrap();
                send(&mut stream, &ClientMessage::Associate(id))
                    .await
                    .unwrap();
                send(
                    &mut stream,
                    &ClientMessage::LoadFromClient(TestStream {
                        group: 0,
                        id: stream_id,
                    }),
                )
                .await
                .unwrap();
                (data, stream)
            }
        })
        .collect()
        .await;

    let (state_tx, state_rx) = watch::channel(TestState::Setup);

    let loaders: Vec<_> = loaders
        .into_iter()
        .map(|(data, stream)| {
            let mut state_rx = state_rx.clone();
            tokio::spawn(async move {
                let mut raw = stream.into_inner();

                wait_for_state(&mut state_rx, TestState::LoadFromClient).await;
                println!("Loading");
                let load_start = Instant::now();

                loop {
                    raw.write(data.as_ref()).await.unwrap();

                    if *state_rx.borrow() != TestState::LoadFromClient {
                        break;
                    }

                    yield_now().await;
                }
                println!("Loading done after {:?}", load_start.elapsed());

                time::sleep(grace).await;
            })
        })
        .collect();

    send(&mut control, &ClientMessage::GetMeasurements).await?;

    let (rx, tx) = control.into_inner().into_split();
    let mut rx = FramedRead::new(rx, codec());
    let mut tx = FramedWrite::new(tx, codec());

    let bandwidth = tokio::spawn(async move {
        let mut bandwidth = Vec::new();

        loop {
            let reply: ServerMessage = receive(&mut rx).await.unwrap();
            match reply {
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

        println!("exiting GetMeasurements");

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

    // Main logic
    let start = Instant::now();

    state_tx.send(TestState::Grace1).unwrap();

    time::sleep(grace).await;

    state_tx.send(TestState::LoadFromClient).unwrap();

    time::sleep(load_duration).await;

    state_tx.send(TestState::Grace2).unwrap();

    time::sleep(grace).await;

    state_tx.send(TestState::End).unwrap();

    let duration = start.elapsed();
    // End

    println!("Test duration {:?}", duration);

    ping_send.await?;
    let pings = ping_recv.await?;

    send(&mut tx, &ClientMessage::Done).await?;

    let bandwidth = bandwidth.await?;

    let bandwidth: Vec<_> = bandwidth
        .iter()
        .filter(|e| e.0.id == 0)
        .map(|e| (e.1, e.2))
        .collect();

    let bandwidth: Vec<_> = (0..bandwidth.len())
        .map(|i| {
            let rate = if i > 0 {
                let bytes = bandwidth[i].1 - bandwidth[i - 1].1;
                let duration = Duration::from_micros(bandwidth[i].0 - bandwidth[i - 1].0);
                let mbits = (bytes as f64 * 8.0) / (1000.0 * 1000.0);
                mbits / duration.as_secs_f64()
            } else {
                0.0
            };
            (bandwidth[i].0, rate)
        })
        .collect();

    graph(pings, bandwidth, duration.as_secs_f64());

    Ok(())
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
            println!("Stopped pinging after {:?}", setup_start.elapsed());
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

fn graph(mut pings: Vec<(Ping, u64)>, bandwidth: Vec<(u64, f64)>, duration: f64) {
    use plotters::prelude::*;

    //println!("band{:#?}", bandwidth);

    let root = BitMapBackend::new("plot.png", (1024, 768)).into_drawing_area();

    root.fill(&WHITE).unwrap();

    let max_latency = pings.iter().map(|d| d.1).max().unwrap_or(100) as f64 / 1000.0;

    let mut max_bandwidth = bandwidth.iter().map(|d| d.1).fold(0. / 0., f64::max);

    if max_bandwidth.is_nan() {
        max_bandwidth = 100.0;
    }

    println!("max_bandwidth{:?}", max_bandwidth);

    println!("max_latency{:?}", max_latency);

    let mut chart = ChartBuilder::on(&root)
        .margin(6)
        .caption("Latency under load", ("sans-serif", 30))
        .set_label_area_size(LabelAreaPosition::Left, 60)
        .set_label_area_size(LabelAreaPosition::Right, 60)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .build_cartesian_2d(0.0..duration, 0.0..max_latency)
        .unwrap()
        .set_secondary_coord(0.0..duration, 0.0..max_bandwidth);

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .x_labels(30)
        .y_desc("Latency (ms)")
        .x_desc("Elapsed time (seconds)")
        .draw()
        .unwrap();
    chart
        .configure_secondary_axes()
        .y_desc("Bandwidth (Mbps)")
        .draw()
        .unwrap();

    pings.sort_by_key(|d| d.0.index);

    chart
        .draw_series(LineSeries::new(
            pings.iter().map(|(ping, latency)| {
                (
                    ping.timestamp as f64 / (1000.0 * 1000.0),
                    *latency as f64 / 1000.0,
                )
            }),
            &BLUE,
        ))
        .unwrap();

    chart
        .draw_secondary_series(LineSeries::new(
            bandwidth
                .iter()
                .map(|(time, rate)| (*time as f64 / (1000.0 * 1000.0), *rate)),
            &RED,
        ))
        .unwrap();

    // To avoid the IO failure being ignored silently, we manually call the present function
    root.present().expect("Unable to write result to file, please make sure 'plotters-doc-data' dir exists under current dir");
}

pub fn test(host: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(test_async(host)).unwrap();
}
