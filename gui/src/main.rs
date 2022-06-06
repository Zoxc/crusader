#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{mem, sync::Arc, time::Duration};

use eframe::{
    egui::{
        self,
        plot::{HLine, Legend, Line, LinkedAxisGroup, Plot, Value, Values},
        Layout, Ui,
    },
    emath::{Align, Vec2},
    epaint::Color32,
};
use library::{
    protocol, serve2,
    test2::{self, to_rates, Config},
};
use tokio::sync::{
    mpsc::{self, error::TryRecvError},
    oneshot,
};

fn main() {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Speed and latency tester",
        options,
        Box::new(|_cc| {
            Box::new(Tester {
                tab: Tab::Client,
                client_state: ClientState::Stopped,
                client: None,
                result: None,
                msgs: Vec::new(),
                server_addr: "localhost".to_owned(),
                download: true,
                upload: false,
                both: false,
                server_state: ServerState::Stopped(None),
                server: None,
                result_saved: None,
                axis: LinkedAxisGroup::x(),
            })
        }),
    );
}

struct Server {
    done: Option<oneshot::Receiver<()>>,
    msgs: Vec<String>,
    rx: mpsc::UnboundedReceiver<String>,
    stop: Option<oneshot::Sender<()>>,
    started: oneshot::Receiver<Result<(), String>>,
}

enum ServerState {
    Stopped(Option<String>),
    Starting,
    Stopping,
    Running,
}

struct Client {
    rx: mpsc::UnboundedReceiver<String>,
    done: oneshot::Receiver<Result<TestResult, String>>,
}

#[derive(PartialEq, Eq)]
enum ClientState {
    Stopped,
    Running,
}

#[derive(PartialEq, Eq)]
enum Tab {
    Client,
    Server,
    Result,
}

struct Tester {
    tab: Tab,
    server_addr: String,
    download: bool,
    upload: bool,
    both: bool,
    server_state: ServerState,
    server: Option<Server>,
    client_state: ClientState,
    client: Option<Client>,
    result: Option<TestResult>,
    result_saved: Option<String>,
    msgs: Vec<String>,
    axis: LinkedAxisGroup,
}

pub struct TestResult {
    result: test2::TestResult,
    download: Vec<(f64, f64)>,
    upload: Vec<(f64, f64)>,
    both: Vec<(f64, f64)>,
    latency: Vec<(f64, f64)>,
    loss: Vec<f64>,
}

pub fn handle_bytes(data: &[(u64, f64)], start: f64) -> Vec<(f64, f64)> {
    to_rates(data)
        .into_iter()
        .map(|(time, speed)| (Duration::from_micros(time).as_secs_f64() - start, speed))
        .collect()
}

impl Drop for Tester {
    fn drop(&mut self) {
        self.server.as_mut().map(|server| {
            mem::take(&mut server.stop).map(|stop| {
                stop.send(()).unwrap();
            });
            mem::take(&mut server.done).map(|done| {
                done.blocking_recv().ok();
            });
        });
    }
}

impl Tester {
    fn client(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let active = self.client_state == ClientState::Stopped;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(active, |ui| {
                ui.label("Server address:");
                ui.text_edit_singleline(&mut self.server_addr);

                if ui.button("Start test").clicked() {
                    self.client_state = ClientState::Running;
                    self.msgs.clear();

                    let config = Config {
                        port: protocol::PORT,
                        streams: 16,
                        grace_duration: 1,
                        load_duration: 1,
                        download: self.download,
                        upload: self.upload,
                        both: self.both,
                        ping_interval: 5,
                        bandwidth_interval: 20,
                        plot_transferred: false,
                        plot_width: None,
                        plot_height: None,
                    };

                    let (signal_done, done) = oneshot::channel();
                    let (tx, rx) = mpsc::unbounded_channel();

                    let ctx = ctx.clone();
                    let ctx_ = ctx.clone();

                    test2::test_callback(
                        config,
                        &self.server_addr,
                        Arc::new(move |msg| {
                            tx.send(msg.to_string()).unwrap();
                            ctx.request_repaint();
                        }),
                        Box::new(move |result| {
                            signal_done
                                .send(result.map(|result| {
                                    let start = result.start.as_secs_f64();
                                    let download =
                                        handle_bytes(&result.combined_download_bytes, start);
                                    let upload = handle_bytes(&result.combined_upload_bytes, start);
                                    let both = handle_bytes(
                                        result.both_bytes.as_deref().unwrap_or(&[]),
                                        start,
                                    );
                                    let latency = result
                                        .pings
                                        .iter()
                                        .filter_map(|(_, sent, latency)| {
                                            latency.map(|latency| {
                                                (
                                                    sent.as_secs_f64() - start,
                                                    latency.as_secs_f64() * 1000.0,
                                                )
                                            })
                                        })
                                        .collect();
                                    let loss = result
                                        .pings
                                        .iter()
                                        .filter_map(|(_, sent, latency)| {
                                            if latency.is_none() {
                                                Some(sent.as_secs_f64() - start)
                                            } else {
                                                None
                                            }
                                        })
                                        .collect();
                                    TestResult {
                                        result,
                                        download,
                                        upload,
                                        both,
                                        latency,
                                        loss,
                                    }
                                }))
                                .map_err(|_| ())
                                .unwrap();
                            ctx_.request_repaint();
                        }),
                    );

                    self.client = Some(Client { done, rx });
                    self.client_state = ClientState::Running;
                    self.result = None;
                    self.result_saved = None;
                }
            });

            ui.collapsing("Options", |ui| {
                ui.add_enabled_ui(active, |ui| {
                    ui.checkbox(&mut self.download, "Download");
                    ui.checkbox(&mut self.upload, "Upload");
                    ui.checkbox(&mut self.both, "Both");
                });
            });
        });
        ui.separator();

        if self.client_state == ClientState::Running {
            let client = self.client.as_mut().unwrap();
            if let Ok(result) = client.done.try_recv() {
                match result {
                    Ok(result) => {
                        self.msgs.push("Test complete.".to_owned());
                        self.result = Some(result);
                        if self.tab == Tab::Client {
                            self.tab = Tab::Result;
                        }
                    }
                    Err(error) => {
                        self.msgs.push(format!("Error: {error}"));
                    }
                }
                self.client_state = ClientState::Stopped;
            }

            while let Ok(msg) = client.rx.try_recv() {
                println!("[Client] {msg}");
                self.msgs.push(msg);
            }
        }

        ui.vertical(|ui| {
            for msg in &self.msgs {
                ui.label(&*msg);
            }
        });
    }

    fn result(&mut self, _ctx: &egui::Context, ui: &mut Ui) {
        if self.result.is_none() {
            ui.label("No result.");
            return;
        }
        let result = self.result.as_ref().unwrap();

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.result_saved.is_none(), |ui| {
                if ui.button("Save as image").clicked() {
                    self.result_saved = Some(test2::save_graph(&result.result, "plot"));
                }
            });
            self.result_saved
                .as_ref()
                .map(|file| ui.label(format!("Saved as: {file}")));
        });
        ui.separator();

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            // Packet loss
            let plot = Plot::new("loss")
                .legend(Legend::default())
                .show_axes([false, false])
                .link_axis(self.axis.clone())
                .height(20.0);

            plot.show(ui, |plot_ui| {
                for loss in &result.loss {
                    plot_ui.hline(HLine::new(*loss).color(Color32::from_rgb(193, 85, 85)))
                }
            });
            ui.label("Packet loss");

            // Latency

            let plot = Plot::new("ping")
                .legend(Legend::default())
                .height(ui.available_height() / 2.0)
                .link_axis(self.axis.clone());

            plot.show(ui, |plot_ui| {
                let latency = result.latency.iter().map(|v| Value::new(v.0 as f64, v.1));
                let latency = Line::new(Values::from_values_iter(latency))
                    .color(Color32::from_rgb(50, 50, 50));

                plot_ui.line(latency);
            });
            ui.label("Latency");

            // Bandwidth
            let plot = Plot::new("result")
                .legend(Legend::default())
                .link_axis(self.axis.clone())
                .set_margin_fraction(Vec2 { x: 0.2, y: 0.0 });

            plot.show(ui, |plot_ui| {
                if result.result.config.download {
                    let download = result.download.iter().map(|v| Value::new(v.0 as f64, v.1));
                    let download = Line::new(Values::from_values_iter(download))
                        .color(Color32::from_rgb(95, 145, 62))
                        .name("Download");

                    plot_ui.line(download);
                }
                if result.result.config.upload {
                    let upload = result.upload.iter().map(|v| Value::new(v.0 as f64, v.1));
                    let upload = Line::new(Values::from_values_iter(upload))
                        .color(Color32::from_rgb(37, 83, 169))
                        .name("Upload");

                    plot_ui.line(upload);
                }
                if result.result.config.both {
                    let both = result.both.iter().map(|v| Value::new(v.0 as f64, v.1));
                    let both = Line::new(Values::from_values_iter(both))
                        .color(Color32::from_rgb(149, 96, 153))
                        .name("Both");

                    plot_ui.line(both);
                }
            });
            ui.label("Bandwidth");
        });
    }

    fn server(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        match self.server_state {
            ServerState::Stopped(ref error) => {
                let button = ui
                    .vertical(|ui| {
                        let button = ui.button("Start server");
                        if let Some(error) = error {
                            ui.separator();
                            ui.label(format!("Unable to start server: {}", error));
                        }
                        button
                    })
                    .inner;

                if button.clicked() {
                    let ctx = ctx.clone();
                    let ctx_ = ctx.clone();
                    let ctx__ = ctx.clone();
                    let (tx, rx) = mpsc::unbounded_channel();
                    let (signal_started, started) = oneshot::channel();
                    let (signal_done, done) = oneshot::channel();

                    let stop = serve2::serve_until(
                        protocol::PORT,
                        Box::new(move |msg| {
                            tx.send(msg.to_string()).unwrap();
                            ctx.request_repaint();
                        }),
                        Box::new(move |result| {
                            signal_started.send(result).unwrap();
                            ctx_.request_repaint();
                        }),
                        Box::new(move || {
                            signal_done.send(()).unwrap();
                            ctx__.request_repaint();
                        }),
                    );

                    self.server = Some(Server {
                        done: Some(done),
                        stop: Some(stop),
                        started,
                        rx,
                        msgs: Vec::new(),
                    });
                    self.server_state = ServerState::Starting;
                };
            }
            ServerState::Running => {
                let server = self.server.as_mut().unwrap();
                let button = ui.button("Stop server");

                ui.separator();

                loop {
                    match server.rx.try_recv() {
                        Ok(msg) => {
                            println!("[Server] {msg}");
                            server.msgs.push(msg);
                        }
                        Err(TryRecvError::Disconnected) => panic!(),
                        Err(TryRecvError::Empty) => break,
                    }
                }

                ui.vertical(|ui| {
                    for msg in &server.msgs {
                        ui.label(&*msg);
                    }
                });

                if button.clicked() {
                    mem::take(&mut server.stop).unwrap().send(()).unwrap();
                    self.server_state = ServerState::Stopping;
                };
            }
            ServerState::Starting => {
                let server = self.server.as_mut().unwrap();

                if let Ok(result) = server.started.try_recv() {
                    if let Err(error) = result {
                        self.server_state = ServerState::Stopped(Some(error));
                    } else {
                        self.server_state = ServerState::Running;
                    }
                }

                ui.add_enabled_ui(false, |ui| {
                    let _ = ui.button("Starting..");
                });
            }
            ServerState::Stopping => {
                if let Ok(()) = self
                    .server
                    .as_mut()
                    .unwrap()
                    .done
                    .as_mut()
                    .unwrap()
                    .try_recv()
                {
                    self.server_state = ServerState::Stopped(None);
                    self.server = None;
                }

                ui.add_enabled_ui(false, |ui| {
                    let _ = ui.button("Stopping..");
                });
            }
        }
    }
}

impl eframe::App for Tester {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Client, "Client");
                ui.selectable_value(&mut self.tab, Tab::Server, "Server");
                ui.selectable_value(&mut self.tab, Tab::Result, "Result");
            });
            ui.separator();

            match self.tab {
                Tab::Client => self.client(ctx, ui),
                Tab::Server => self.server(ctx, ui),
                Tab::Result => self.result(ctx, ui),
            }
        });
    }
}
