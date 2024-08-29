use crate::{common::is_unicast_link_local, protocol, serve::State, version};
#[cfg(feature = "client")]
use anyhow::anyhow;
use anyhow::bail;
#[cfg(target_family = "unix")]
use nix::net::if_::InterfaceFlags;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket};
use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr},
    str::FromStr,
    sync::Arc,
};
use tokio::net::UdpSocket;

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
    Discover {
        peer: bool,
    },
    Server {
        peer: bool,
        port: u16,
        protocol_version: u64,
        software_version: String,
        hostname: Option<String>,
        label: Option<String>,
        ips: Vec<[u8; 16]>,
    },
}

#[cfg(feature = "client")]
pub struct Server {
    pub at: String,
    pub socket: SocketAddr,
    pub software_version: String,
}

fn interfaces() -> Vec<u32> {
    let mut _result = vec![0];

    #[cfg(target_family = "unix")]
    {
        if let Ok(interfaces) = nix::ifaddrs::getifaddrs() {
            for interface in interfaces {
                if interface.flags.contains(InterfaceFlags::IFF_LOOPBACK) {
                    continue;
                }
                if !interface.flags.contains(InterfaceFlags::IFF_MULTICAST) {
                    continue;
                }
                if let Some(addr) = interface.address.as_ref().and_then(|i| i.as_sockaddr_in6()) {
                    if !is_unicast_link_local(addr.ip()) {
                        continue;
                    }
                    _result.push(addr.scope_id());
                }
            }
        }
    }

    _result
}

#[cfg(feature = "client")]
pub async fn locate(peer_server: bool) -> Result<Server, anyhow::Error> {
    use crate::common::fresh_socket_addr;
    use std::{net::SocketAddrV6, time::Duration};
    use tokio::time::timeout;

    fn handle_packet(
        peer_server: bool,
        packet: &[u8],
        src: SocketAddr,
    ) -> Result<Server, anyhow::Error> {
        let data: Data = bincode::deserialize(packet)?;
        if data.hello != Hello::new() {
            bail!("Wrong hello");
        }
        if let Message::Server {
            peer,
            port,
            protocol_version,
            software_version,
            hostname,
            ips: _,
            label: _,
        } = data.message
        {
            if peer != peer_server {
                bail!("Wrong server kind");
            }
            if protocol_version != protocol::VERSION {
                bail!("Wrong protocol");
            }
            let socket = fresh_socket_addr(src, port);

            let at = hostname
                .map(|hostname| format!("`{hostname}` {socket}"))
                .unwrap_or(socket.to_string());

            Ok(Server {
                at,
                socket,
                software_version,
            })
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
        message: Message::Discover { peer: peer_server },
    };

    let buf = bincode::serialize(&data)?;

    let ip = Ipv6Addr::from_str("ff02::1").unwrap();

    let mut any = false;
    for interface in interfaces() {
        if socket
            .send_to(&buf, SocketAddrV6::new(ip, DISCOVER_PORT, 0, interface))
            .await
            .is_ok()
        {
            any = true;
        }
    }
    if !any {
        bail!("Failed to send any discovery multicast packets");
    }

    let find = async {
        let mut buf = [0; 1500];
        loop {
            if let Ok((len, src)) = socket.recv_from(&mut buf).await {
                if let Ok(server) = handle_packet(peer_server, &buf[..len], src) {
                    return server;
                }
            }
        }
    };

    timeout(Duration::from_secs(1), find).await.map_err(|_| {
        if peer_server {
            anyhow!("Failed to locate local latency peer")
        } else {
            anyhow!("Failed to locate local server")
        }
    })
}

pub fn serve(state: Arc<State>, port: u16, peer_server: bool) -> Result<(), anyhow::Error> {
    async fn handle_packet(
        port: u16,
        peer_server: bool,
        hostname: &Option<String>,
        packet: &[u8],
        socket: &UdpSocket,
        src: SocketAddr,
    ) -> Result<(), anyhow::Error> {
        match src {
            SocketAddr::V6(src) if is_unicast_link_local(*src.ip()) => (),
            _ => bail!("Unexpected source"),
        }
        let data: Data = bincode::deserialize(packet)?;
        if data.hello != Hello::new() {
            bail!("Wrong hello");
        }
        if let Message::Discover { peer } = data.message {
            if peer != peer_server {
                return Ok(());
            }
            let data = Data {
                hello: Hello::new(),
                message: Message::Server {
                    peer,
                    port,
                    protocol_version: protocol::VERSION,
                    software_version: version(),
                    hostname: hostname.clone(),
                    label: None,
                    ips: Vec::new(),
                },
            };
            let buf = bincode::serialize(&data)?;
            socket.send_to(&buf, src).await?;
        }
        Ok(())
    }

    let hostname = hostname::get()
        .ok()
        .and_then(|n| n.into_string().ok())
        .filter(|n| n != "localhost");

    let socket = Socket::new(Domain::IPV6, socket2::Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), DISCOVER_PORT).into())?;
    let socket: std::net::UdpSocket = socket.into();
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;

    let ip = Ipv6Addr::from_str("ff02::1").unwrap();

    let mut any = false;
    for interface in interfaces() {
        if socket.join_multicast_v6(&ip, interface).is_ok() {
            any = true;
        }
    }
    if !any {
        bail!("Failed to join any multicast groups");
    }

    tokio::spawn(async move {
        let mut buf = [0; 1500];
        loop {
            if let Ok((len, src)) = socket.recv_from(&mut buf).await {
                handle_packet(port, peer_server, &hostname, &buf[..len], &socket, src)
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
