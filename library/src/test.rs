use bincode::serialize_into;
use plotters::prelude::*;
use rand::{prelude::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, ErrorKind, Read, Write},
    net::{TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
pub(crate) enum ServerMessage {
    NewClient(u64),
}

#[derive(Serialize, Deserialize)]
pub(crate) enum ClientMessage {
    NewClient,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct LoadConfig {
    from_server: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Ping {
    index: u32,
    timestamp: u128,
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

pub fn test(host: &str) {
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
            (
                data.clone(),
                TcpStream::connect((host, 30481)).expect("unable to bind TCP socket"),
            )
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
                timestamp: current.as_nanos(),
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

            let latency = current.as_nanos().saturating_sub(ping.timestamp);

            //println!("pingy {:?}", Duration::from_nanos(latency as u64));

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

                let config = LoadConfig { from_server: false };

                stream.write(&bincode::serialize(&config).unwrap()).unwrap();

                stream.read(&mut [0]).unwrap();

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

    sender.join().unwrap();
    let mut pings = receiver.join().unwrap();
    loaders
        .into_iter()
        .for_each(|loader| loader.join().unwrap());

    let root = BitMapBackend::new("plot.png", (1024, 768)).into_drawing_area();

    root.fill(&WHITE).unwrap();

    let max_latency = pings.iter().map(|d| d.1).max().unwrap_or_default() as f64 / (1000.0 * 1000.0);

    let mut chart = ChartBuilder::on(&root)
        .margin(6)
        .caption(
            "Latency under load",
            ("sans-serif", 30),
        )
        .set_label_area_size(LabelAreaPosition::Left, 60)
        .set_label_area_size(LabelAreaPosition::Right, 60)
        .set_label_area_size(LabelAreaPosition::Bottom, 40)
        .build_cartesian_2d(
            0.0..(secs as f64),
            0.0..max_latency,
        ).unwrap()
       .set_secondary_coord(
            0.0..(secs as f64),
            0.0..1000.0f64,
        );

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .x_labels(30)
        .y_desc("Latency (ms)")
        .x_desc("Elapsed time (seconds)")
        .draw().unwrap();
  chart
        .configure_secondary_axes()
        .y_desc("Bandwidth (Mbps)")
        .draw().unwrap();

    pings.sort_by_key(|d| d.0.index);

    chart.draw_series(LineSeries::new(
        pings.iter().map(|(ping, latency)| (ping.timestamp as f64 / (1000.0 * 1000.0 * 1000.0), *latency as f64 / (1000.0 * 1000.0))),
        &BLUE,
    )).unwrap();
    
    chart.draw_secondary_series(LineSeries::new(
        pings.iter().map(|(ping, _)| (ping.timestamp as f64 / (1000.0 * 1000.0 * 1000.0), 100.0)),
        &RED,
    )).unwrap();

    // To avoid the IO failure being ignored silently, we manually call the present function
    root.present().expect("Unable to write result to file, please make sure 'plotters-doc-data' dir exists under current dir");

    println!("Test complete");
}
