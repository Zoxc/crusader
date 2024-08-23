use crate::{protocol, serve::State};
use anyhow::bail;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket};
use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
};
use tokio::net::UdpSocket;
#[cfg(feature = "client")]
use {crate::common::Msg, anyhow::anyhow};

pub const DISCOVER_PORT: u16 = protocol::PORT + 2;
pub const DISCOVER_VERSION: u64 = 0;

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct Hello {
    magic: u64,
    pub version: u64,
}

impl Hello {
    pub fn new() -> Self {
        Hello {
            magic: protocol::MAGIC,
            version: DISCOVER_VERSION,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Data {
    hello: Hello,
    message: Message,
}

#[derive(Serialize, Deserialize, Debug)]
enum Message {
    Discover,
    Server {
        port: u16,
        protocol_version: u64,
        software_version: String,
        hostname: Option<String>,
    },
}

#[cfg(feature = "client")]
pub async fn locate(msg: Msg) -> Result<SocketAddr, anyhow::Error> {
    use crate::common::fresh_socket_addr;
    use std::time::Duration;
    use tokio::time::timeout;

    fn handle_packet(
        msg: &Msg,
        packet: &[u8],
        src: SocketAddr,
    ) -> Result<SocketAddr, anyhow::Error> {
        let data: Data = bincode::deserialize(packet)?;
        if data.hello != Hello::new() {
            bail!("Wrong hello");
        }
        if let Message::Server {
            port,
            protocol_version,
            software_version,
            hostname,
        } = data.message
        {
            if protocol_version != protocol::VERSION {
                bail!("Wrong protocol");
            }
            let server = fresh_socket_addr(src, port);

            let at = hostname
                .map(|hostname| format!("`{hostname}` {server}"))
                .unwrap_or(server.to_string());

            msg(&format!(
                "Found server at {at} running version {software_version}"
            ));

            Ok(server)
        } else {
            bail!("Wrong message")
        }
    }

    let socket = Socket::new(Domain::IPV6, socket2::Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0).into())?;
    let socket: std::net::UdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    socket.set_broadcast(true)?;

    let data = Data {
        hello: Hello::new(),
        message: Message::Discover,
    };

    let buf = bincode::serialize(&data)?;

    socket.send_to(&buf, ("ff02::1", DISCOVER_PORT)).await?;

    let find = async {
        let mut buf = [0; 1500];
        loop {
            if let Ok((len, src)) = socket.recv_from(&mut buf).await {
                if let Ok(server) = handle_packet(&msg, &buf[..len], src) {
                    return server;
                }
            }
        }
    };

    timeout(Duration::from_secs(1), find)
        .await
        .map_err(|_| anyhow!("Failed to locate local server"))
}

pub fn serve(state: Arc<State>, port: u16) -> Result<(), anyhow::Error> {
    async fn handle_packet(
        port: u16,
        hostname: &Option<String>,
        packet: &[u8],
        socket: &UdpSocket,
        src: SocketAddr,
    ) -> Result<(), anyhow::Error> {
        let data: Data = bincode::deserialize(packet)?;
        if data.hello != Hello::new() {
            bail!("Wrong hello");
        }
        if let Message::Discover = data.message {
            let data = Data {
                hello: Hello::new(),
                message: Message::Server {
                    port,
                    protocol_version: protocol::VERSION,
                    software_version: crate::VERSION.to_owned(),
                    hostname: hostname.clone(),
                },
            };
            let buf = bincode::serialize(&data)?;
            socket.send_to(&buf, src).await?;
        }
        Ok(())
    }

    let hostname = hostname::get().ok().and_then(|n| n.into_string().ok());

    let socket = Socket::new(Domain::IPV6, socket2::Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), DISCOVER_PORT).into())?;
    let socket: std::net::UdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    socket.join_multicast_v6(&Ipv6Addr::from_str("ff02::1")?, 0)?;

    tokio::spawn(async move {
        let mut buf = [0; 1500];
        loop {
            if let Ok((len, src)) = socket.recv_from(&mut buf).await {
                handle_packet(port, &hostname, &buf[..len], &socket, src)
                    .await
                    .map_err(|error| {
                        (state.msg)(&format!("Unable to handle discovery packet: {:?}", error));
                    })
                    .ok();
            }
        }
    });

    Ok(())
}
