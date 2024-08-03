use crate::plot::save_graph_to_mem;
use crate::test::{test_async, PlotConfig};
use crate::{test::Config, with_time, LIB_VERSION};
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
use std::io::Cursor;
use std::time::Duration;
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};
use tokio::sync::mpsc::unbounded_channel;
use tokio::{net::TcpListener, signal, task};

struct Env {
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
    server: String,
    download: bool,
    upload: bool,
    both: bool,
    port: u16,

    streams: u64,

    stream_stagger: f64,

    load_duration: f64,

    grace_duration: f64,
    latency_sample_rate: u64,
    throughput_sample_rate: u64,
    latency_peer: Option<String>,
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
        both: args.both,
        ping_interval: Duration::from_millis(args.latency_sample_rate),
        throughput_interval: Duration::from_millis(args.throughput_sample_rate),
    };

    let (msg_tx, mut msg_rx) = unbounded_channel();

    let tester = tokio::spawn(async move {
        let msg = Arc::new(move |msg: &str| {
            let msg = with_time(msg);
            msg_tx.send(msg.clone()).ok();
            task::spawn_blocking(move || println!("{}", msg));
        });
        test_async(
            config,
            &args.server,
            args.latency_peer.as_deref(),
            msg.clone(),
        )
        .await
        .map_err(|err| {
            msg(&format!("Client failed: {}", err));
            anyhow!("Client failed")
        })
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

    let result = tester.await??;

    socket
        .send(Message::Text(
            json!({
                "type": "result",
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

        result.save_to_writer(&mut data);
        data
    })
    .await?;
    socket.send(Message::Binary(data)).await?;

    (state.msg)(&format!("Remote client running from {}", who.ip()));
    Ok(())
}

async fn listen(state: Arc<Env>, listener: TcpListener) {
    #[cfg(debug_assertions)]
    async fn root() -> Html<String> {
        Html(
            std::fs::read_to_string("crusader-lib/src/remote.html")
                .unwrap_or(include_str!("remote.html").to_string()),
        )
    }

    #[cfg(not(debug_assertions))]
    async fn root() -> Html<&'static str> {
        Html(include_str!("remote.html"))
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
    let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
    let state = Arc::new(Env { msg });

    task::spawn(listen(state.clone(), v4));

    (state.msg)(&format!(
        "Remote{} version {} running...",
        if cfg!(debug_assertions) {
            " (debugging enabled)"
        } else {
            ""
        },
        LIB_VERSION
    ));
    (state.msg)(&format!("Address http://localhost:{}", port));

    Ok(())
}

pub fn run(port: u16) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        serve_async(
            port,
            Box::new(|msg: &str| {
                let msg = msg.to_owned();
                task::spawn_blocking(move || println!("{}", with_time(&msg)));
            }),
        )
        .await
        .unwrap();
        signal::ctrl_c().await.unwrap();
        println!("{}", with_time("Remote server aborting..."));
    });
}
