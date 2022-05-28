use std::{net::UdpSocket, thread};

pub fn serve() {
    let pinger = thread::spawn(move || {
        let socket = UdpSocket::bind("127.0.0.1:30481").expect("unable to bind UDP socket");

        let mut buf = [0; 400];

        loop {
            let (len, src) = socket.recv_from(&mut buf).expect("unable to get udp ping");

            // Redeclare `buf` as slice of the received data and send reverse data back to origin.
            let buf = &mut buf[..len];
            socket
                .send_to(buf, &src)
                .expect("unable to reply to udp ping");
        }
    });

    println!("Serving..");

    pinger.join()
    .unwrap();
}
