#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{mem, sync::Arc};

use eframe::egui;
use library::{
    protocol, serve2,
    test2::{self, Config},
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
                client_tab: true,
                client_state: ClientState::Stopped,
                client: None,
                msgs: Vec::new(),
                server_addr: "localhost".to_owned(),
                download: true,
                upload: true,
                both: true,
                server_state: ServerState::Stopped(None),
                server: None,
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
    done: oneshot::Receiver<Result<(), String>>,
}

#[derive(PartialEq, Eq)]
enum ClientState {
    Stopped,
    Running,
    Result,
}

struct Tester {
    client_tab: bool,
    server_addr: String,
    download: bool,
    upload: bool,
    both: bool,
    server_state: ServerState,
    server: Option<Server>,
    client_state: ClientState,
    client: Option<Client>,
    msgs: Vec<String>,
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

impl eframe::App for Tester {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.client_tab, true, "Client");
                ui.selectable_value(&mut self.client_tab, false, "Server");
            });
            ui.separator();
            if self.client_tab {
                let active = self.client_state == ClientState::Stopped
                    || self.client_state == ClientState::Result;
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
                                load_duration: 5,
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
                                    signal_done.send(result).unwrap();
                                    ctx_.request_repaint();
                                }),
                            );

                            self.client = Some(Client { done, rx });
                            self.client_state = ClientState::Running;
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
                            Ok(()) => {
                                self.client_state = ClientState::Result;
                            }
                            Err(error) => {
                                self.msgs.push(format!("Error: {error}"));
                                self.client_state = ClientState::Stopped;
                            }
                        }
                    }

                    while let Ok(msg) = client.rx.try_recv() {
                        println!("[Client] {msg}");
                        self.msgs.push(msg);
                    }
                }

                if self.client_state == ClientState::Result {
                    ui.label("Results!");
                } else {
                    ui.vertical(|ui| {
                        for msg in &self.msgs {
                            ui.label(&*msg);
                        }
                    });
                }
            } else {
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
        });
    }
}
