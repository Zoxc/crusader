use crate::common::{interface_ips, Config};
use crate::plot::save_graph_to_mem;
use crate::test::{test_async, timed, PlotConfig};
use crate::{version, with_time};
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Error;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{header, HeaderValue, Response};
use axum::{
    extract::{ConnectInfo, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use image::ImageFormat;
use serde::Deserialize;
use serde_json::json;
use socket2::{Domain, Protocol, Socket};
use std::io::{Cursor, ErrorKind};
use std::net::{IpAddr, Ipv6Addr};
use std::thread;
use std::time::Duration;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::net::TcpSocket;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio::{net::TcpListener, signal, task};

struct Env {
    live_reload: bool,
    msg: Box<dyn Fn(&str) + Send + Sync>,
}

async fn ws_client(
    State(state): State<Arc<Env>>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        handle_client(state, socket, addr).await.ok();
    })
}

#[derive(Deserialize, Debug)]
struct TestArgs {
    server: Option<String>,
    download: bool,
    upload: bool,
    bidirectional: bool,
    port: u16,

    streams: u64,

    stream_stagger: f64,

    load_duration: f64,

    grace_duration: f64,
    latency_sample_interval: u64,
    throughput_sample_interval: u64,
    latency_peer: bool,
    latency_peer_server: Option<String>,
}

async fn handle_client(
    state: Arc<Env>,
    mut socket: WebSocket,
    who: SocketAddr,
) -> Result<(), Error> {
    let args: TestArgs = match socket.recv().await.ok_or(anyhow!("No request"))?? {
        Message::Text(request) => serde_json::from_str(&request)?,
        _ => bail!("unexpected message"),
    };
    let config = Config {
        port: args.port,
        streams: args.streams,
        stream_stagger: Duration::from_secs_f64(args.stream_stagger),
        grace_duration: Duration::from_secs_f64(args.grace_duration),
        load_duration: Duration::from_secs_f64(args.load_duration),
        download: args.download,
        upload: args.upload,
        bidirectional: args.bidirectional,
        ping_interval: Duration::from_millis(args.latency_sample_interval),
        throughput_interval: Duration::from_millis(args.throughput_sample_interval),
    };

    (state.msg)(&format!("Remote client ({}) test started", who.ip()));

    let (msg_tx, mut msg_rx) = unbounded_channel();

    let tester = tokio::spawn(async move {
        let msg = Arc::new(move |msg: &str| {
            let msg = with_time(msg);
            msg_tx.send(msg.clone()).ok();
            task::spawn_blocking(move || println!("{}", msg));
        });
        let result = test_async(
            config,
            args.server.as_deref(),
            args.latency_peer
                .then_some(args.latency_peer_server.as_deref()),
            msg.clone(),
        )
        .await
        .map_err(|err| {
            msg(&format!("Client failed: {}", err));
            anyhow!("Client failed")
        });
        (result, timed(""))
    });

    while let Some(msg) = msg_rx.recv().await {
        socket
            .send(Message::Text(
                json!({
                    "type": "log",
                    "message": msg,
                })
                .to_string(),
            ))
            .await?;
    }

    let (result, time) = tester.await?;
    let result = result?;

    socket
        .send(Message::Text(
            json!({
                "type": "result",
                "time": time,
            })
            .to_string(),
        ))
        .await?;

    let (result, plot) = task::spawn_blocking(move || -> Result<_, anyhow::Error> {
        let mut data = Cursor::new(Vec::new());

        let plot = save_graph_to_mem(&PlotConfig::default(), &result.to_test_result())?;
        plot.write_to(&mut data, ImageFormat::Png)?;
        Ok((result, data.into_inner()))
    })
    .await??;

    socket.send(Message::Binary(plot)).await?;

    let data = task::spawn_blocking(move || {
        let mut data = Vec::new();

        result.save_to_writer(&mut data)?;
        Ok::<_, anyhow::Error>(data)
    })
    .await??;
    socket.send(Message::Binary(data)).await?;

    (state.msg)(&format!("Remote client ({}) test complete", who.ip()));
    Ok(())
}

async fn listen(state: Arc<Env>, listener: TcpListener) {
    async fn root(State(state): State<Arc<Env>>) -> Html<String> {
        if state.live_reload {
            if let Ok(data) = std::fs::read_to_string("crusader-lib/src/remote.html") {
                return Html(data);
            }
        }

        Html(include_str!("remote.html").to_string())
    }

    async fn vue() -> Response<Body> {
        #[cfg(debug_assertions)]
        let body: Body = include_str!("../assets/vue.js").into();
        #[cfg(not(debug_assertions))]
        let body: Body = include_str!("../assets/vue.prod.js").into();
        (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript"),
            )],
            body,
        )
            .into_response()
    }

    let app = Router::new()
        .route("/", get(root))
        .route("/assets/vue.js", get(vue))
        .route("/api/client", get(ws_client))
        .with_state(state);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn serve_async(port: u16, msg: Box<dyn Fn(&str) + Send + Sync>) -> Result<(), Error> {
    let live_reload = cfg!(debug_assertions)
        && std::fs::read_to_string("crusader-lib/src/remote.html")
            .map(|file| *file == *include_str!("remote.html"))
            .unwrap_or_default();

    if live_reload {
        (msg)(&format!(
            "Live reload of crusader-lib/src/remote.html enabled",
        ));
    }

    let state = Arc::new(Env { live_reload, msg });

    let v6 = Socket::new(Domain::IPV6, socket2::Type::STREAM, Some(Protocol::TCP))?;
    v6.set_only_v6(true)?;
    let v6: std::net::TcpStream = v6.into();
    v6.set_nonblocking(true)?;
    let v6 = TcpSocket::from_std_stream(v6);
    v6.bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))
        .map_err(|error| {
            if let ErrorKind::AddrInUse = error.kind() {
                anyhow!(
                    "Failed to bind TCP port, maybe another Crusader instance is already running"
                )
            } else {
                error.into()
            }
        })?;
    let v6 = v6.listen(1024)?;

    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;

    task::spawn(listen(state.clone(), v6));
    task::spawn(listen(state.clone(), v4));

    (state.msg)(&format!(
        "Remote{} version {} running...",
        if cfg!(debug_assertions) {
            " (debugging enabled)"
        } else {
            ""
        },
        version()
    ));

    for (name, ip) in interface_ips() {
        let addr = match ip {
            IpAddr::V6(ip) => format!("[{ip}]"),
            IpAddr::V4(ip) => ip.to_string(),
        };
        (state.msg)(&format!("Address on `{name}`: http://{addr}:{port}"));
    }

    Ok(())
}

pub fn serve_until(
    port: u16,
    msg: Box<dyn Fn(&str) + Send + Sync>,
    started: Box<dyn FnOnce(Result<(), String>) + Send>,
    done: Box<dyn FnOnce() + Send>,
) -> Result<oneshot::Sender<()>, anyhow::Error> {
    let (tx, rx) = oneshot::channel();

    let rt = tokio::runtime::Runtime::new()?;

    thread::spawn(move || {
        rt.block_on(async move {
            match serve_async(port, msg).await {
                Ok(()) => {
                    started(Ok(()));
                    rx.await.ok();
                }
                Err(error) => started(Err(error.to_string())),
            }
        });

        done();
    });

    Ok(tx)
}

pub fn run(port: u16) -> Result<(), anyhow::Error> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        serve_async(
            port,
            Box::new(|msg: &str| {
                let msg = msg.to_owned();
                task::spawn_blocking(move || println!("{}", with_time(&msg)));
            }),
        )
        .await?;
        signal::ctrl_c().await?;
        println!("{}", with_time("Remote server aborting..."));
        Ok(())
    })
}
