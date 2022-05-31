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
use tokio::net::TcpStream;
use tokio::task::yield_now;
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{Framed, FramedRead, FramedWrite};

use crate::protocol::{codec, receive, send, ClientMessage, Hello, Ping, ServerMessage};

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

    send(&mut control, &ClientMessage::NewClient).await?;
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

    let loading_streams = 1;

    let grace = 2;
    let load_duration = 9;
    let interval_ms = 1000;
    let secs = load_duration * 1 + grace * 2;
    let duration = Duration::from_secs(secs);

    let loaders: Vec<_> = stream::iter(0..loading_streams)
        .then(|_| {
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
                send(&mut stream, &ClientMessage::LoadFromClient)
                    .await
                    .unwrap();
                (data, stream)
            }
        })
        .collect()
        .await;

    let start = Instant::now();

    let end = start + duration;

    let sender = tokio::spawn(async move {
        let udp_socket = udp_socket2;
        let mut interval = time::interval(Duration::from_millis(interval_ms));

        let mut buf = [0; 64];
        let mut index: u32 = 0;

        loop {
            interval.tick().await;

            let current = start.elapsed();

            if current > duration {
                println!("Stopped pinging after {:?}", current);
                break;
            }

            let ping = Ping {
                index,
                timestamp: current.as_micros() as u64,
            };

            let mut cursor = Cursor::new(&mut buf[..]);
            bincode::serialize_into(&mut cursor, &ping).unwrap();
            let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

            udp_socket.send(buf).await.expect("unable to udp ping");

            index += 1;
        }
    });

    let receiver = tokio::spawn(async move {
        let mut storage =
            Vec::with_capacity(((secs as f64) * (1000.0 / interval_ms as f64) * 1.5) as usize);
        let mut buf = [0; 64];
        let deadline = time::sleep_until(time::Instant::from_std(end)).fuse();
        pin_mut!(deadline);

        loop {
            let result = {
                let packet = udp_socket.recv(&mut buf).fuse();
                pin_mut!(packet);

                select! {
                    result = packet => result,
                    _ = deadline => break,
                }
            };

            let current = start.elapsed();
            let len = result.unwrap();
            let buf = &mut buf[..len];
            let ping: Ping = bincode::deserialize(buf).unwrap();

            let latency = (current.as_micros() as u64).saturating_sub(ping.timestamp);

            println!("Ping {}", latency as f64 / 1000.0);

            storage.push((ping, latency));
        }

        storage
    });

    let loaders: Vec<_> = loaders
        .into_iter()
        .map(|(data, stream)| {
            tokio::spawn(async move {
                time::sleep(Duration::from_secs(grace)).await;
                println!("Loading");

                let mut raw = stream.into_inner();

                let load_start = Instant::now();

                loop {
                    raw.write(data.as_ref()).await.unwrap();

                    if start.elapsed() > Duration::from_secs(grace + load_duration) {
                        break;
                    }

                    yield_now().await;
                }
                println!("Loading done after {:?}", load_start.elapsed());
            })
        })
        .collect();

    send(&mut control, &ClientMessage::GetMeasurements).await?;

    let (rx, tx) = control.into_inner().into_split();
    let mut rx = FramedRead::new(rx, codec());
    let mut tx = FramedWrite::new(tx, codec());

    tokio::spawn(async move {
        loop {
            let reply: ServerMessage = receive(&mut rx).await.unwrap();
            match reply {
                ServerMessage::Measure {
                    time: _,
                    duration,
                    bytes,
                } => {
                    let mbits = (bytes as f64 * 8.0) / 1000.0 / 1000.0;
                    let rate = mbits / Duration::from_micros(duration).as_secs_f64();
                    println!("Rate: {:>10.2} Mbps, Bytes: {}", rate, bytes);
                }
                ServerMessage::MeasurementsDone => break,
                _ => panic!("Unexpected message {:?}", reply),
            };
        }
    });

    sender.await?;
    receiver.await?;

    send(&mut tx, &ClientMessage::Done).await?;

    Ok(())
}

pub fn test(host: &str) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(test_async(host)).unwrap();
}
