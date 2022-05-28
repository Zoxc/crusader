use bincode::deserialize_from;
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, UdpSocket},
    thread,
    time::{Duration, Instant},
};

use crate::test::LoadConfig;

pub fn serve() {
    let socket = UdpSocket::bind("127.0.0.1:30481").expect("unable to bind UDP socket");

    let pinger = thread::spawn(move || {
        let mut buf = [0; 256];

        loop {
            let (len, src) = socket.recv_from(&mut buf).expect("unable to get udp ping");

            // Redeclare `buf` as slice of the received data and send reverse data back to origin.
            let buf = &mut buf[..len];
            socket
                .send_to(buf, &src)
                .expect("unable to reply to udp ping");
        }
    });

    let listener = TcpListener::bind("127.0.0.1:30481").expect("unable to bind TCP socket");

    let listener = thread::spawn(move || {
        for stream in listener.incoming() {
            stream
                .map(|mut stream| {
                    println!("Spawning..");
                    thread::spawn(move || {
                        let mut buf = [0; 1024 * 1024];
                        println!("Serving.. {}", stream.peer_addr().unwrap());
                        let config: LoadConfig = deserialize_from(&mut stream).unwrap();

                        stream.write(&[0]).unwrap();

                        stream
                            .set_nonblocking(true)
                            .expect("failed to make stream non-blocking");

                        let start = Instant::now();
                        let mut data = 0;
                        let mut previous = start;

                        let interval = 1000;

                        loop {
                            let current = Instant::now();
                            let elapsed = current.saturating_duration_since(previous);
                            if elapsed > Duration::from_millis(interval) {
                                let mbits = (data as f64 * 8.0) / 1000.0 / 1000.0;
                                let rate = mbits / elapsed.as_secs_f64();
                                println!("Rate: {:>10.2} Mbps, Bytes: {}", rate, data);
                                data = 0;
                                previous = current;
                            }

                            match stream.read(&mut buf[..]) {
                                Ok(0) => break,
                                Err(err) => {
                                    if err.kind() == ErrorKind::WouldBlock {
                                        thread::sleep(Duration::from_millis(interval / 10));
                                    } else {
                                        break;
                                    }
                                }
                                Ok(v) => {
                                    data += v;
                                }
                            }
                        }
                        let dur = start.elapsed();
                        println!("Client end {}. Loaded for {:?}", stream.peer_addr().unwrap(), dur);
                    })
                })
                .unwrap();
        }
    });

    println!("Serving..");

    pinger.join().unwrap();
    listener.join().unwrap();
}
