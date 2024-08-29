#[cfg(feature = "client")]
use crate::common::{Config, Msg};
use crate::protocol::PeerLatency;
use crate::serve::State;
use crate::{
    common::{hello, measure_latency, ping_recv, ping_send, TestState},
    protocol::{codec, receive, send, ClientMessage, RawLatency, ServerMessage},
};
use anyhow::bail;
use anyhow::Context;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio::{
    net::{self},
    time,
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
#[cfg(feature = "client")]
use {crate::common::connect, crate::discovery};

#[cfg(feature = "client")]
pub struct Peer {
    msg: Msg,
    tx: FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    rx: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
}

#[cfg(feature = "client")]
impl Peer {
    pub async fn start(&mut self) -> Result<(), anyhow::Error> {
        let reply: ServerMessage = receive(&mut self.rx).await?;
        match reply {
            ServerMessage::PeerReady { server_latency } => {
                (self.msg)(&format!(
                    "Peer idle latency to server {:.2} ms",
                    Duration::from_nanos(server_latency).as_secs_f64() * 1000.0
                ));
            }
            _ => bail!("Unexpected message {:?}", reply),
        };
        send(&mut self.tx, &ClientMessage::PeerStart).await?;
        let reply: ServerMessage = receive(&mut self.rx).await?;
        match reply {
            ServerMessage::PeerStarted => (),
            _ => bail!("Unexpected message {:?}", reply),
        };
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), anyhow::Error> {
        send(&mut self.tx, &ClientMessage::PeerStop).await?;
        Ok(())
    }

    pub async fn complete(mut self) -> Result<(bool, Vec<PeerLatency>), anyhow::Error> {
        let reply: ServerMessage = receive(&mut self.rx).await?;
        match reply {
            ServerMessage::PeerDone {
                overload,
                latencies,
            } => Ok((overload, latencies)),
            _ => bail!("Unexpected message {:?}", reply),
        }
    }
}

#[cfg(feature = "client")]
pub async fn connect_to_peer(
    config: Config,
    server: SocketAddr,
    latency_peer_server: Option<&str>,
    estimated_duration: Duration,
    msg: Msg,
) -> Result<Peer, anyhow::Error> {
    let control = if let Some(server) = latency_peer_server {
        connect((server, config.port), "latency peer").await?
    } else {
        let server = discovery::locate(true).await?;
        msg(&format!(
            "Found peer at {} running version {}",
            server.at, server.software_version
        ));
        connect(server.socket, "latency peer").await?
    };
    control.set_nodelay(true)?;

    let peer_server = control.peer_addr()?;

    msg(&format!("Connected to peer {}", peer_server));

    let (rx, tx) = control.into_split();
    let mut control_rx = FramedRead::new(rx, codec());
    let mut control_tx = FramedWrite::new(tx, codec());

    hello(&mut control_tx, &mut control_rx).await?;

    send(
        &mut control_tx,
        &ClientMessage::NewPeer {
            server: match server.ip() {
                IpAddr::V4(ip) => ip.to_ipv6_mapped(),
                IpAddr::V6(ip) => ip,
            }
            .octets(),
            port: config.port,
            ping_interval: config.ping_interval.as_millis() as u64,
            estimated_duration: estimated_duration.as_millis(),
        },
    )
    .await?;

    let reply: ServerMessage = receive(&mut control_rx).await?;
    match reply {
        ServerMessage::NewPeer => (),
        _ => bail!("Unexpected message {:?}", reply),
    };

    Ok(Peer {
        msg,
        rx: control_rx,
        tx: control_tx,
    })
}

pub async fn run_peer(
    state: Arc<State>,
    server: SocketAddr,
    ping_interval: Duration,
    estimated_duration: Duration,
    stream_rx: &mut FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    stream_tx: &mut FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
) -> Result<(), anyhow::Error> {
    let control = net::TcpStream::connect(server)
        .await
        .context("Peer failed to connect to server")?;
    control.set_nodelay(true)?;

    let server = control.peer_addr()?;

    (state.msg)(&format!("Peer connected to server {}", server));

    let (rx, tx) = control.into_split();
    let mut control_rx = FramedRead::new(rx, codec());
    let mut control_tx = FramedWrite::new(tx, codec());

    hello(&mut control_tx, &mut control_rx).await?;

    send(&mut control_tx, &ClientMessage::NewClient).await?;

    let setup_start = Instant::now();

    let reply: ServerMessage = receive(&mut control_rx).await?;
    let id = match reply {
        ServerMessage::NewClient(Some(id)) => id,
        ServerMessage::NewClient(None) => bail!("Server was unable to create client"),
        _ => bail!("Unexpected message {:?}", reply),
    };

    send(stream_tx, &ServerMessage::NewPeer).await?;

    let local_udp = if server.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };

    let mut ping_index = 0;

    let (latency, server_time_offset, mut control_rx) = measure_latency(
        id,
        &mut ping_index,
        &mut control_tx,
        control_rx,
        server,
        local_udp,
        setup_start,
    )
    .await?;

    (state.msg)(&format!(
        "Peer idle latency to server {:.2} ms",
        latency.as_secs_f64() * 1000.0
    ));

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let (state_tx, state_rx) = watch::channel((TestState::Setup, setup_start));

    send(&mut control_tx, &ClientMessage::GetMeasurements).await?;

    let measures = tokio::spawn(async move {
        let mut latencies = Vec::new();
        let overload_;

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await?;
            match reply {
                ServerMessage::LatencyMeasures(measures) => {
                    latencies.extend(measures.into_iter());
                }
                ServerMessage::MeasurementsDone { overload } => {
                    overload_ = overload;
                    break;
                }
                _ => bail!("Unexpected message {:?}", reply),
            };
        }

        Ok((latencies, overload_))
    });

    send(
        stream_tx,
        &ServerMessage::PeerReady {
            server_latency: latency.as_nanos() as u64,
        },
    )
    .await?;

    let reply: ClientMessage = receive(stream_rx).await?;
    match reply {
        ClientMessage::PeerStart => (),
        _ => bail!("Unexpected message {:?}", reply),
    };

    let ping_start_index = ping_index;
    let ping_send = tokio::spawn(ping_send(
        ping_index,
        id,
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
        estimated_duration,
    ));

    let ping_recv = tokio::spawn(ping_recv(
        state_rx.clone(),
        setup_start,
        udp_socket2.clone(),
        ping_interval,
        estimated_duration,
    ));

    send(stream_tx, &ServerMessage::PeerStarted).await?;

    // Wait for client to complete test
    let reply: ClientMessage = receive(stream_rx).await?;
    match reply {
        ClientMessage::PeerStop => (),
        _ => bail!("Unexpected message {:?}", reply),
    };

    state_tx.send((TestState::End, Instant::now())).ok();

    // Wait for pings to return
    time::sleep(Duration::from_millis(500)).await;

    state_tx.send((TestState::EndPingRecv, Instant::now())).ok();

    let pings_sent = ping_send.await??;
    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;
    send(&mut control_tx, &ClientMessage::Done).await?;

    let mut pongs = ping_recv.await??;

    let (mut latencies, server_overload) = measures.await??;

    latencies.sort_by_key(|d| d.index);
    pongs.sort_by_key(|d| d.0.index);
    let pings: Vec<_> = pings_sent
        .into_iter()
        .enumerate()
        .map(|(index, sent)| {
            let index = index as u64 + ping_start_index;
            let mut latency = latencies
                .binary_search_by_key(&index, |e| e.index)
                .ok()
                .map(|ping| RawLatency {
                    total: None,
                    up: Duration::from_micros(
                        latencies[ping].time.wrapping_add(server_time_offset),
                    )
                    .saturating_sub(sent),
                });

            latency.as_mut().map(|latency| {
                pongs
                    .binary_search_by_key(&index, |e| e.0.index)
                    .ok()
                    .map(|ping| {
                        latency.total = Some(pongs[ping].1.saturating_sub(sent));
                    });
            });

            PeerLatency {
                sent: (sent.as_micros() as u64).wrapping_sub(server_time_offset),
                latency,
            }
        })
        .collect();

    send(
        stream_tx,
        &ServerMessage::PeerDone {
            overload: server_overload,
            latencies: pings,
        },
    )
    .await?;

    Ok(())
}
