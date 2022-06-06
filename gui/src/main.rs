#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{
    mem,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use eframe::egui;
use library::{protocol, serve2};
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
                client: true,
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
    done: Arc<AtomicBool>,
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

struct Tester {
    client: bool,
    server_addr: String,
    download: bool,
    upload: bool,
    both: bool,
    server_state: ServerState,
    server: Option<Server>,
}

impl eframe::App for Tester {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.client, true, "Client");
                ui.selectable_value(&mut self.client, false, "Server");
            });
            ui.separator();
            if self.client {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Server address:");
                    ui.text_edit_singleline(&mut self.server_addr);
                    ui.button("Start test");

                    ui.collapsing("Options", |ui| {
                        ui.checkbox(&mut self.download, "Download");
                        ui.checkbox(&mut self.upload, "Upload");
                        ui.checkbox(&mut self.both, "Both");
                    });
                });
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
                            let done = Arc::new(AtomicBool::new(false));
                            let done_ = done.clone();
                            let ctx = ctx.clone();
                            let ctx_ = ctx.clone();
                            let (tx, rx) = mpsc::unbounded_channel();
                            let (signal_started, started) = oneshot::channel();

                            let stop = serve2::serve_until(
                                protocol::PORT,
                                Box::new(move |msg| tx.send(msg.to_string()).unwrap()),
                                Box::new(move |result| {
                                    signal_started.send(result).unwrap();
                                    ctx.request_repaint();
                                }),
                                Box::new(move || {
                                    done_.store(true, Ordering::Release);
                                    ctx_.request_repaint();
                                }),
                            );

                            self.server = Some(Server {
                                done,
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
                        if self.server.as_ref().unwrap().done.load(Ordering::Acquire) {
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
