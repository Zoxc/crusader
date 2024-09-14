use crate::{
    protocol::{receive, send, ClientMessage, Hello, Ping, ServerMessage},
    serve::OnDrop,
};
use anyhow::{anyhow, bail, Context};
use bytes::{Bytes, BytesMut};
use futures::{pin_mut, select, FutureExt, Sink, Stream};
use rand::Rng;
use rand::{rngs::StdRng, SeedableRng};
use std::{
    error::Error,
    io::Cursor,
    net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    join,
    net::{
        self,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream, ToSocketAddrs, UdpSocket,
    },
    sync::{
        oneshot,
        watch::{self, error::RecvError},
    },
    task::yield_now,
    time::{self, timeout, Instant},
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[cfg(feature = "client")]
pub(crate) type Msg = Arc<dyn Fn(&str) + Send + Sync>;

#[allow(unused)]
#[derive(PartialEq, Eq, Debug, Clone, Copy, PartialOrd, Ord)]
pub(crate) enum TestState {
    Setup,
    Grace1,
    LoadFromClient,
    Grace2,
    LoadFromServer,
    Grace3,
    LoadFromBoth,
    Grace4,
    End,
    EndPingRecv,
}

#[cfg(feature = "client")]
#[derive(Copy, Clone, PartialEq)]
pub struct Config {
    pub download: bool,
    pub upload: bool,
    pub bidirectional: bool,
    pub port: u16,
    pub load_duration: Duration,
    pub grace_duration: Duration,
    pub streams: u64,
    pub stream_stagger: Duration,
    pub ping_interval: Duration,
    pub throughput_interval: Duration,
}

pub async fn connect<A: ToSocketAddrs>(addr: A, name: &str) -> Result<TcpStream, anyhow::Error> {
    match timeout(Duration::from_secs(8), net::TcpStream::connect(addr)).await {
        Ok(v) => v.with_context(|| format!("Failed to connect to {name}")),
        Err(_) => bail!("Timed out trying to connect to {name}. Is the {name} running?"),
    }
}

pub fn interface_ips() -> Vec<(String, IpAddr)> {
    let mut _result = Vec::new();

    #[cfg(target_family = "unix")]
    {
        use nix::net::if_::InterfaceFlags;

        if let Ok(interfaces) = nix::ifaddrs::getifaddrs() {
            for interface in interfaces {
                if interface.flags.contains(InterfaceFlags::IFF_LOOPBACK) {
                    continue;
                }
                if !interface.flags.contains(InterfaceFlags::IFF_RUNNING) {
                    continue;
                }
                if let Some(addr) = interface.address.as_ref().and_then(|i| i.as_sockaddr_in()) {
                    _result.push((interface.interface_name.clone(), IpAddr::V4(addr.ip())));
                }
                if let Some(addr) = interface.address.as_ref().and_then(|i| i.as_sockaddr_in6()) {
                    if is_unicast_link_local(addr.ip()) {
                        continue;
                    }
                    _result.push((interface.interface_name.clone(), IpAddr::V6(addr.ip())));
                }
            }
        }
    }

    #[cfg(target_family = "windows")]
    {
        if let Ok(adapters) = ipconfig::get_adapters() {
            for adapter in adapters {
                if adapter.oper_status() != ipconfig::OperStatus::IfOperStatusUp {
                    continue;
                }
                for &addr in adapter.ip_addresses() {
                    if let IpAddr::V6(ip) = addr {
                        if is_unicast_link_local(ip) {
                            continue;
                        }
                    }
                    if addr.is_loopback() {
                        continue;
                    }
                    _result.push((adapter.friendly_name().to_owned(), addr));
                }
            }
        }
    }

    _result
}

pub fn is_unicast_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

pub fn fresh_socket_addr(socket: SocketAddr, port: u16) -> SocketAddr {
    match socket {
        SocketAddr::V4(socket) => SocketAddr::V4(SocketAddrV4::new(*socket.ip(), port)),
        SocketAddr::V6(socket) => {
            if let Some(ip) = socket.ip().to_ipv4_mapped() {
                return SocketAddr::V4(SocketAddrV4::new(ip, port));
            }
            SocketAddr::V6(SocketAddrV6::new(*socket.ip(), port, 0, socket.scope_id()))
        }
    }
}

pub fn inherit_local(socket: SocketAddr, ip: IpAddr, port: u16) -> SocketAddr {
    if let SocketAddr::V6(socket) = socket {
        if let IpAddr::V6(ip) = ip {
            if is_unicast_link_local(ip) {
                return SocketAddr::V6(SocketAddrV6::new(ip, port, 0, socket.scope_id()));
            }
        }
    }

    SocketAddr::new(ip, port)
}

pub(crate) fn data() -> Vec<u8> {
    let mut vec = Vec::with_capacity(128 * 1024);
    let mut rng = StdRng::from_seed([
        18, 141, 186, 158, 195, 76, 244, 56, 219, 131, 65, 128, 250, 63, 228, 44, 233, 34, 9, 51,
        13, 72, 230, 131, 223, 240, 124, 77, 103, 238, 103, 186,
    ]);
    for _ in 0..vec.capacity() {
        vec.push(rng.gen())
    }
    vec
}

pub(crate) async fn read_data(
    stream: TcpStream,
    buffer: &mut [u8],
    bytes: Arc<AtomicU64>,
    until: Instant,
    writer_done: oneshot::Receiver<()>,
) -> Result<bool, anyhow::Error> {
    stream.set_linger(Some(Duration::from_secs(0))).ok();

    let reading_done = Arc::new(AtomicBool::new(false));

    // Set `reading_done` to true 2 minutes after the load should terminate.
    let reading_done_ = reading_done.clone();
    tokio::spawn(async move {
        time::sleep_until(until + Duration::from_secs(120)).await;
        reading_done_.store(true, Ordering::Release);
    });

    // Set `reading_done` to true after 5 seconds of not receiving data.
    let reading_done_ = reading_done.clone();
    let bytes_ = bytes.clone();
    tokio::spawn(async move {
        writer_done.await.ok();

        let mut current = bytes_.load(Ordering::Acquire);
        let mut i = 0;
        loop {
            time::sleep(Duration::from_millis(100)).await;

            if reading_done_.load(Ordering::Acquire) {
                break;
            }

            let now = bytes_.load(Ordering::Acquire);

            if now != current {
                i = 0;
                current = now;
            } else {
                i += 1;

                if i > 50 {
                    reading_done_.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });

    // Set `reading_done` to true on exit to terminate the spawned task.
    let reading_done_ = reading_done.clone();
    let _on_drop = OnDrop(|| {
        reading_done_.store(true, Ordering::Release);
    });

    loop {
        if let Ok(Err(err)) = time::timeout(Duration::from_millis(50), stream.readable()).await {
            if err.kind() == std::io::ErrorKind::ConnectionReset
                || err.kind() == std::io::ErrorKind::ConnectionAborted
            {
                return Ok(false);
            } else {
                return Err(err.into());
            }
        }

        loop {
            if reading_done.load(Ordering::Acquire) {
                return Ok(true);
            }

            match stream.try_read(buffer) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    bytes.fetch_add(n as u64, Ordering::Release);
                    yield_now().await;
                }
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::WouldBlock {
                        break;
                    } else if err.kind() == std::io::ErrorKind::ConnectionReset
                        || err.kind() == std::io::ErrorKind::ConnectionAborted
                    {
                        return Ok(false);
                    } else {
                        return Err(err.into());
                    }
                }
            }
        }
    }
}

pub(crate) async fn write_data(
    stream: TcpStream,
    data: &[u8],
    until: Instant,
) -> Result<(), anyhow::Error> {
    stream.set_nodelay(false).ok();
    stream.set_linger(Some(Duration::from_secs(0))).ok();

    let done = Arc::new(AtomicBool::new(false));
    let done_ = done.clone();

    tokio::spawn(async move {
        time::sleep_until(until).await;
        done.store(true, Ordering::Release);
    });

    loop {
        if let Ok(Err(err)) = time::timeout(Duration::from_millis(50), stream.writable()).await {
            if err.kind() == std::io::ErrorKind::ConnectionReset
                || err.kind() == std::io::ErrorKind::ConnectionAborted
            {
                break;
            } else {
                return Err(err.into());
            }
        }

        if done_.load(Ordering::Acquire) {
            break;
        }
        match stream.try_write(data) {
            Ok(_) => (),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::WouldBlock {
                } else if err.kind() == std::io::ErrorKind::ConnectionReset
                    || err.kind() == std::io::ErrorKind::ConnectionAborted
                {
                    break;
                } else {
                    return Err(err.into());
                }
            }
        }

        yield_now().await;
    }

    std::mem::drop(stream);

    Ok(())
}

pub(crate) async fn hello<
    T: Sink<Bytes> + Unpin,
    R: Stream<Item = Result<BytesMut, RE>> + Unpin,
    RE,
>(
    tx: &mut T,
    rx: &mut R,
) -> Result<(), anyhow::Error>
where
    T::Error: Error + Send + Sync + 'static,
    RE: Error + Send + Sync + 'static,
{
    let hello = Hello::new();

    send(tx, &hello).await.context("Sending hello")?;
    let server_hello: Hello = receive(rx).await.context("Receiving hello")?;

    if hello != server_hello {
        bail!(
            "Mismatched server hello, got {:?}, expected {:?}",
            server_hello,
            hello
        );
    }

    Ok(())
}

pub(crate) fn udp_handle(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            if e.raw_os_error() == Some(libc::ENOBUFS) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

async fn ping_measure_send(
    mut index: u64,
    id: u64,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    samples: u32,
) -> Result<(Vec<Duration>, u64), anyhow::Error> {
    let mut storage = Vec::with_capacity(samples as usize);
    let mut buf = [0; 64];

    let mut interval = time::interval(Duration::from_millis(10));

    for _ in 0..samples {
        interval.tick().await;

        let current = setup_start.elapsed();

        let ping = Ping { id, index };

        index += 1;

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping)?;
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        socket.send(buf).await?;

        storage.push(current);
    }

    Ok((storage, index))
}

async fn ping_measure_recv(
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    samples: u32,
) -> Result<Vec<(Ping, Duration)>, anyhow::Error> {
    let mut storage = Vec::with_capacity(samples as usize);
    let mut buf = [0; 64];

    let end = time::sleep(Duration::from_millis(10) * samples + Duration::from_millis(1000)).fuse();
    pin_mut!(end);

    loop {
        let result = {
            let packet = socket.recv(&mut buf).fuse();
            pin_mut!(packet);

            select! {
                result = packet => result,
                _ = end => break,
            }
        };

        let current = setup_start.elapsed();
        let len = result?;
        let buf = buf
            .get_mut(..len)
            .ok_or_else(|| anyhow!("Pong too large"))?;
        let ping: Ping = bincode::deserialize(buf)?;

        storage.push((ping, current));
    }

    Ok(storage)
}

pub(crate) async fn measure_latency(
    id: u64,
    ping_index: &mut u64,
    mut control_tx: &mut FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    mut control_rx: FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    server: SocketAddr,
    local_udp: SocketAddr,
    setup_start: Instant,
) -> Result<
    (
        Duration,
        u64,
        FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    ),
    anyhow::Error,
> {
    send(&mut control_tx, &ClientMessage::GetMeasurements).await?;

    let latencies = tokio::spawn(async move {
        let mut latencies = Vec::new();

        loop {
            let reply: ServerMessage = receive(&mut control_rx).await?;
            match reply {
                ServerMessage::LatencyMeasures(measures) => {
                    latencies.extend(measures.into_iter());
                }
                ServerMessage::MeasurementsDone { .. } => break,
                _ => bail!("Unexpected message {:?}", reply),
            };
        }

        Ok((latencies, control_rx))
    });

    let udp_socket = Arc::new(net::UdpSocket::bind(local_udp).await?);
    udp_socket.connect(server).await?;
    let udp_socket2 = udp_socket.clone();

    let samples = 50;

    let ping_start_index = *ping_index;
    let ping_send = tokio::spawn(ping_measure_send(
        ping_start_index,
        id,
        setup_start,
        udp_socket,
        samples,
    ));

    let ping_recv = tokio::spawn(ping_measure_recv(setup_start, udp_socket2, samples));

    let (sent, recv) = join!(ping_send, ping_recv);

    send(&mut control_tx, &ClientMessage::StopMeasurements).await?;

    let (mut latencies, control_rx) = latencies.await??;

    let (sent, new_ping_index) = sent??;
    *ping_index = new_ping_index;
    let mut recv = recv??;

    latencies.sort_by_key(|d| d.index);
    recv.sort_by_key(|d| d.0.index);
    let mut pings: Vec<(Duration, Duration, u64)> = sent
        .into_iter()
        .enumerate()
        .filter_map(|(index, sent)| {
            let index = index as u64 + ping_start_index;
            let latency = latencies
                .binary_search_by_key(&index, |e| e.index)
                .ok()
                .map(|ping| latencies[ping].time);

            latency.and_then(|time| {
                recv.binary_search_by_key(&index, |e| e.0.index)
                    .ok()
                    .map(|ping| (sent, recv[ping].1 - sent, time))
            })
        })
        .collect();
    if pings.is_empty() {
        bail!("Unable to measure latency to server");
    }

    pings.sort_by_key(|d| d.1);

    let (sent, latency, server_time) = pings[pings.len() / 2];

    let server_pong = sent + latency / 2;

    let server_offset = (server_pong.as_micros() as u64).wrapping_sub(server_time);

    Ok((latency, server_offset, control_rx))
}

pub(crate) async fn ping_send(
    mut ping_index: u64,
    id: u64,
    state_rx: watch::Receiver<(TestState, Instant)>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
    estimated_duration: Duration,
) -> Result<Vec<Duration>, anyhow::Error> {
    let mut storage = Vec::with_capacity(
        ((estimated_duration.as_secs_f64() + 2.0) * (1000.0 / interval.as_millis() as f64) * 1.5)
            as usize,
    );
    let mut buf = [0; 64];

    let mut interval = time::interval(interval);

    loop {
        interval.tick().await;

        if state_rx.borrow().0 >= TestState::End {
            break;
        }

        let current = setup_start.elapsed();

        let ping = Ping {
            id,
            index: ping_index,
        };

        ping_index += 1;

        let mut cursor = Cursor::new(&mut buf[..]);
        bincode::serialize_into(&mut cursor, &ping).unwrap();
        let buf = &cursor.get_ref()[0..(cursor.position() as usize)];

        udp_handle(socket.send(buf).await.map(|_| ())).context("Unable to send UDP ping packet")?;

        storage.push(current);
    }

    Ok(storage)
}

pub(crate) async fn ping_recv(
    mut state_rx: watch::Receiver<(TestState, Instant)>,
    setup_start: Instant,
    socket: Arc<UdpSocket>,
    interval: Duration,
    estimated_duration: Duration,
) -> Result<Vec<(Ping, Duration)>, anyhow::Error> {
    let mut storage = Vec::with_capacity(
        ((estimated_duration.as_secs_f64() + 2.0) * (1000.0 / interval.as_millis() as f64) * 1.5)
            as usize,
    );
    let mut buf = [0; 64];

    let end = wait_for_state(&mut state_rx, TestState::EndPingRecv).fuse();
    pin_mut!(end);

    loop {
        let result = {
            let packet = socket.recv(&mut buf).fuse();
            pin_mut!(packet);

            select! {
                result = packet => result,
                _ = end => break,
            }
        };

        let current = setup_start.elapsed();
        let len = result?;
        let buf = buf
            .get_mut(..len)
            .ok_or_else(|| anyhow!("Pong too large"))?;
        let ping: Ping = bincode::deserialize(buf)?;

        storage.push((ping, current));
    }

    Ok(storage)
}

pub(crate) async fn wait_for_state(
    state_rx: &mut watch::Receiver<(TestState, Instant)>,
    state: TestState,
) -> Result<Instant, RecvError> {
    loop {
        {
            let current = state_rx.borrow_and_update();
            if current.0 == state {
                return Ok(current.1);
            }
        }
        state_rx.changed().await?;
    }
}
