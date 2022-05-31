use bincode::serialize_into;
use bytes::{Bytes, BytesMut};
use futures::future::FutureExt;
use futures::{sink::SinkExt, stream::StreamExt, Sink, Stream};
use plotters::prelude::*;
use rand::{prelude::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    io::{Cursor, ErrorKind, Write},
    net::{SocketAddr, TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tokio::net::{self, TcpSocket};
use tokio_util::codec::{length_delimited, Framed, LengthDelimitedCodec};

use crate::protocol::{ClientMessage, Hello, Ping, ServerMessage};

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

fn hello(mut stream: &mut TcpStream) {
    let hello = Hello::new();

    serialize_into(&mut stream, &hello).unwrap();
    let server_hello: Hello = bincode::deserialize_from(&mut stream).unwrap();

    if hello != server_hello {
        panic!(
            "Mismatched server hello, got {:?}, expected {:?}",
            server_hello, hello
        );
    }
}
/*
async fn connect_to_server(server: &str) -> Result<(net::TcpSocket, SocketAddr), Box<dyn Error>> {
    for server in net::lookup_host((server, 30481)).await? {
        TcpSocket::n
        if let Some(socket) = net::TcpSocket::connect(self, server).await.ok() {
            println!("Connected to server {}", server);
            return (socket, server);
        }
    }

    Err("Unable to lookup server")
}*/

fn codec() -> LengthDelimitedCodec {
    length_delimited::Builder::new()
        .little_endian()
        .length_field_type::<u64>()
        .new_codec()
}

async fn send<S: Sink<Bytes> + Unpin>(
    sink: &mut S,
    value: &impl Serialize,
) -> Result<(), Box<dyn Error>>
where
    S::Error: Error + 'static,
{
    Ok(sink.send(bincode::serialize(value)?.into()).await?)
}

async fn receive<S: Stream<Item = Result<BytesMut, E>> + Unpin, T: for<'a> Deserialize<'a>, E>(
    stream: &mut S,
) -> Result<T, Box<dyn Error>>
where
    E: Error + 'static,
{
    let bytes = stream.next().await.ok_or("Expected object")??;
    Ok(bincode::deserialize(&bytes)?)
}

async fn hello2<S: Sink<Bytes> + Stream<Item = Result<BytesMut, S::Error>> + Unpin>(
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
    let control = net::TcpStream::connect((server, 30481)).await?;

    let server = control.peer_addr()?;

    println!("Connected to server {}", server);

    let mut control = Framed::new(control, codec());

    hello2(&mut control).await?;

    Ok(())
}

pub fn test2(host: &str) {
    // Create the runtime
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Spawn the root task
    rt.block_on(test_async(host));
}

pub fn test(host: &str) {
    let mut control = TcpStream::connect((host, 30481)).expect("unable to bind control TCP socket");

    hello(&mut control);

    serialize_into(&mut control, &ClientMessage::NewClient).unwrap();
    let reply: ServerMessage = bincode::deserialize_from(&mut control).unwrap();
    let id = match reply {
        ServerMessage::NewClient(id) => id,
        _ => panic!("Unexpected message {:?}", reply),
    };

    let socket = UdpSocket::bind("127.0.0.1:0").expect("unable to bind UDP socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    socket
        .connect((host, 30481))
        .expect("unable to connect UDP socket");

    let socket = Arc::new(socket);
    let socket2 = socket.clone();

    let data = Arc::new(data());

    println!("loaders start..");

    let loading_streams = 1;

    let loaders: Vec<_> = (0..loading_streams)
        .map(|_| {
            let mut stream = TcpStream::connect((host, 30481)).expect("unable to bind TCP socket");
            hello(&mut stream);
            serialize_into(&mut control, &ClientMessage::Associate(id)).unwrap();
            (data.clone(), stream)
        })
        .collect();

    println!("loaders end..");

    let grace = 2;
    let load_duration = 3;
    let interval_ms = 1;
    let secs = load_duration * 1 + grace * 2;
    let duration = Duration::from_secs(secs);

    let start = Instant::now();

    let sender = thread::spawn(move || {
        let mut buf = [0; 64];
        let mut index: u32 = 0;

        loop {
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
            serialize_into(&mut cursor, &ping).unwrap();
            let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

            socket.send(buf).expect("unable to udp ping");

            thread::sleep(Duration::from_millis(interval_ms));

            index += 1;
        }
    });

    let socket = socket2;

    let receiver = thread::spawn(move || {
        let mut storage =
            Vec::with_capacity(((secs as f64) * (1000.0 / interval_ms as f64) * 1.5) as usize);
        let mut buf = [0; 64];

        loop {
            let result = socket.recv(&mut buf);
            let current = start.elapsed();

            let len = match result {
                Ok(len) => len,
                Err(err) => match err.kind() {
                    ErrorKind::WouldBlock | ErrorKind::TimedOut => {
                        if current > duration + Duration::from_secs(1) {
                            break;
                        }

                        continue;
                    }
                    _ => panic!("Unable to receive UDP ping {}", err),
                },
            };

            let buf = &mut buf[..len];
            let ping: Ping = bincode::deserialize(buf).unwrap();

            let latency = (current.as_micros() as u64).saturating_sub(ping.timestamp);

            storage.push((ping, latency));
        }

        storage
    });

    let loaders: Vec<_> = loaders
        .into_iter()
        .map(|(data, mut stream)| {
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(grace));
                println!("Loading");

                serialize_into(&mut stream, &ClientMessage::LoadFromClient).unwrap();

                let reply: ServerMessage = bincode::deserialize_from(&mut stream).unwrap();
                match reply {
                    ServerMessage::WaitingOnLoad => (),
                    _ => panic!("Unexpected message {:?}", reply),
                }

                let load_start = Instant::now();

                loop {
                    stream.write(data.as_ref()).unwrap();

                    if start.elapsed() > Duration::from_secs(grace + load_duration) {
                        break;
                    }
                }
                println!("Loading done after {:?}", load_start.elapsed());
            })
        })
        .collect();

    serialize_into(&mut control, &ClientMessage::GetMeasurements).unwrap();

    loop {
        let reply: ServerMessage = bincode::deserialize_from(&mut control).unwrap();
        match reply {
            ServerMessage::Measure {} => (id),
            ServerMessage::MeasurementsDone => break,
            _ => panic!("Unexpected message {:?}", reply),
        };
    }

    sender.join().unwrap();
    let mut pings = receiver.join().unwrap();
    loaders
        .into_iter()
        .for_each(|loader| loader.join().unwrap());

    serialize_into(&mut control, &ClientMessage::Done).unwrap();

    let root = BitMapBackend::new("plot.png", (1024, 768)).into_drawing_area();

    root.fill(&WHITE).unwrap();

    let max_latency =
        pings.iter().map(|d| d.1).max().unwrap_or_default() as f64 / (1000.0 * 1000.0);

    let mut chart = ChartBuilder::on(&root)
        .margin(6)
        .caption("Latency under load", ("sans-serif", 30))
        .set_label_area_size(LabelAreaPosition::Left, 60)
        .set_label_area_size(LabelAreaPosition::Right, 60)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .build_cartesian_2d(0.0..(secs as f64), 0.0..max_latency)
        .unwrap()
        .set_secondary_coord(0.0..(secs as f64), 0.0..1000.0f64);

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
                    ping.timestamp as f64 / (1000.0 * 1000.0 * 1000.0),
                    *latency as f64 / (1000.0 * 1000.0),
                )
            }),
            &BLUE,
        ))
        .unwrap();

    chart
        .draw_secondary_series(LineSeries::new(
            pings
                .iter()
                .map(|(ping, _)| (ping.timestamp as f64 / (1000.0 * 1000.0 * 1000.0), 100.0)),
            &RED,
        ))
        .unwrap();

    // To avoid the IO failure being ignored silently, we manually call the present function
    root.present().expect("Unable to write result to file, please make sure 'plotters-doc-data' dir exists under current dir");

    println!("Test complete");
}
