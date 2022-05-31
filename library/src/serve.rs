use bincode::{deserialize_from, serialize_into};
use parking_lot::Mutex;
use std::error::Error;
use std::{
    collections::HashMap,
    io::{ErrorKind, Read},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::protocol::{self, ClientMessage, ServerMessage};

struct Client {
    created: Instant,
}

struct State {
    clients: Mutex<HashMap<u64, Arc<Client>>>,
}

fn client(state: Arc<State>, mut stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let hello = protocol::Hello::new();

    let client_hello: protocol::Hello = deserialize_from(&mut stream)?;

    serialize_into(&mut stream, &hello)?;

    if hello != client_hello {
        println!(
            "Client {} had invalid hello {:?}, expected {:?}",
            stream.peer_addr()?,
            client_hello,
            hello
        );
        return Ok(());
    }

    println!("Serving {}, version {}", stream.peer_addr()?, hello.version);

    let mut client = None;

    loop {
        let request: ClientMessage = bincode::deserialize_from(&mut stream)?;
        match request {
            ClientMessage::NewClient => {
                let client = Arc::new(Client {
                    created: Instant::now(),
                });
                let id = Arc::as_ptr(&client) as u64;
                state.clients.lock().insert(id, client);
                serialize_into(&mut stream, &ServerMessage::NewClient(id))?;
            }
            ClientMessage::Associate(id) => {
                client = Some(
                    state
                        .clients
                        .lock()
                        .get(&id)
                        .ok_or_else(|| "Unable to assoicate client")?,
                );
            }
            ClientMessage::GetMeasurements => {
                //thread::spawn(f)
            }
            ClientMessage::LoadFromClient => {
                let mut buf = [0; 1024 * 1024];

                serialize_into(&mut stream, &ServerMessage::WaitingOnLoad)?;

                stream.set_nonblocking(true)?;

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
                println!("Client end {}. Loaded for {:?}", stream.peer_addr()?, dur);
                return Ok(());
            }
            ClientMessage::Done => {
                return Ok(());
            }
            _ => {
                println!(
                    "Unexpected request {:?} from client {}",
                    request,
                    stream.peer_addr()?
                );
                return Ok(());
            }
        };
    }
}

pub fn serve() {
    let state = Arc::new(State {
        clients: Mutex::new(HashMap::new()),
    });

    let state2 = state.clone();

    thread::spawn(move || loop {
        {
            let mut clients = state2.clients.lock();
            let remove: Vec<_> = clients
                .iter()
                .filter(|(_, client)| client.created.elapsed() > Duration::from_secs(300))
                .map(|(id, _)| *id)
                .collect();
            for client in remove {
                clients.remove(&client);
            }
        }

        thread::sleep(Duration::from_secs(300));
    });

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
                .map(|stream| {
                    println!("Spawning..");
                    let state = state.clone();
                    thread::spawn(move || {
                        let addr = stream.peer_addr().unwrap();
                        client(state, stream).map_err(|error| {
                            println!("Error from client {}: {}", addr, error);
                        })
                    })
                })
                .unwrap();
        }
    });

    println!("Serving..");

    pinger.join().unwrap();
    listener.join().unwrap();
}
