use bincode::serialize_into;
use serde::{Deserialize, Serialize};
use std::{
    io::{Cursor, Write},
    net::UdpSocket,
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
pub(crate) struct Ping {
    index: u32,
    timestamp: u128,
}

pub fn test(host: &str) {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("unable to bind UDP socket");

    socket
        .connect((host, 30481))
        .expect("unable to connect UDP socket");

    let start = Instant::now();
    let duration = Duration::from_secs(20);

    let sender = thread::spawn(move || {
        
        let mut buf = [0; 64];
        let mut index: u32 = 0;

        loop {
            let current = Instant::now().saturating_duration_since(start);

            if current > duration {
                break;
            }

            let ping = Ping {
                index,
                timestamp: current.as_nanos(),
            };

            let mut cursor = Cursor::new(&mut buf[..]);
            serialize_into(&mut cursor, &ping).unwrap();
            let buf = &buf[0..(cursor.position() as usize)];

            socket.send(buf).expect("unable to udp ping");

            thread::sleep(Duration::from_secs(10));

            index += 1;
        }
    });

    let receiver =  thread::spawn(move || {
        let mut buf = [0; 64];
     
        loop {
            let len = socket.recv(&mut buf).expect("unable to receive udp ping");
            let buf = &mut buf[..len];
            let ping: Ping = bincode::deserialize(buf).unwrap();

            println!("pingy {:?}", Duration::from_nanos(ping.timestamp as u64));

            let current = Instant::now().saturating_duration_since(start);

            if current > duration + Duration::from_secs(1) {
                break;
            }
        }
    });

    pinger.join().unwrap();
    receiver.join().unwrap();
}
