use crate::{Tab, Tester};
use crusader_lib::{
    file_format::RawResult,
    protocol,
    test::{self},
    with_time, Config,
};
use eframe::{
    egui::{self, Grid, ScrollArea, TextEdit, Ui},
    emath::{vec2, Align},
};
use serde::{Deserialize, Serialize};
use std::{mem, sync::Arc, time::Duration};
use tokio::sync::{
    mpsc::{self},
    oneshot,
};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct ClientSettings {
    pub server: String,
    pub download: bool,
    pub upload: bool,
    pub bidirectional: bool,
    pub streams: u64,
    pub load_duration: f64,
    pub grace_duration: f64,
    pub stream_stagger: f64,
    pub latency_sample_interval: u64,
    pub throughput_sample_interval: u64,
    pub latency_peer: bool,
    pub latency_peer_server: String,
}

impl ClientSettings {
    fn config(&self) -> Config {
        Config {
            port: protocol::PORT,
            streams: self.streams,
            grace_duration: Duration::from_secs_f64(self.grace_duration),
            load_duration: Duration::from_secs_f64(self.load_duration),
            stream_stagger: Duration::from_secs_f64(self.stream_stagger),
            download: self.download,
            upload: self.upload,
            bidirectional: self.bidirectional,
            ping_interval: Duration::from_millis(self.latency_sample_interval),
            throughput_interval: Duration::from_millis(self.throughput_sample_interval),
        }
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            server: String::new(),
            download: true,
            upload: true,
            bidirectional: true,
            streams: 8,
            load_duration: 10.0,
            grace_duration: 2.0,
            stream_stagger: 0.0,
            latency_sample_interval: 5,
            throughput_sample_interval: 60,
            latency_peer: false,
            latency_peer_server: String::new(),
        }
    }
}

pub struct Client {
    rx: mpsc::UnboundedReceiver<String>,
    pub done: Option<oneshot::Receiver<Option<Result<RawResult, String>>>>,
    pub abort: Option<oneshot::Sender<()>>,
}

#[derive(PartialEq, Eq)]
pub enum ClientState {
    Stopped,
    Stopping,
    Running,
}

impl Tester {
    fn start_client(&mut self, ctx: &egui::Context) {
        self.save_settings();
        self.msgs.clear();
        self.msg_scrolled = 0;

        let (signal_done, done) = oneshot::channel();
        let (tx, rx) = mpsc::unbounded_channel();

        let ctx = ctx.clone();
        let ctx_ = ctx.clone();

        let abort = test::test_callback(
            self.settings.client.config(),
            (!self.settings.client.server.trim().is_empty())
                .then_some(&self.settings.client.server),
            self.settings.client.latency_peer.then_some(
                (!self.settings.client.latency_peer_server.trim().is_empty())
                    .then_some(&self.settings.client.latency_peer_server),
            ),
            Arc::new(move |msg| {
                tx.send(with_time(msg)).unwrap();
                ctx.request_repaint();
            }),
            Box::new(move |result| {
                signal_done.send(result).map_err(|_| ()).unwrap();
                ctx_.request_repaint();
            }),
        );

        self.client = Some(Client {
            done: Some(done),
            rx,
            abort: Some(abort),
        });
        self.client_state = ClientState::Running;
    }

    pub fn client(&mut self, ctx: &egui::Context, ui: &mut Ui, compact: bool) {
        let active = self.client_state == ClientState::Stopped;

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(active, |ui| {
                ui.label("Server address:");
                let response = ui.add(
                    TextEdit::singleline(&mut self.settings.client.server)
                        .hint_text("(Locate local server)"),
                );
                if self.client_state == ClientState::Stopped
                    && response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.start_client(ctx)
                }
            });

            match self.client_state {
                ClientState::Running => {
                    if ui.button("Stop test").clicked() {
                        let client = self.client.as_mut().unwrap();
                        mem::take(&mut client.abort).unwrap().send(()).unwrap();
                        self.client_state = ClientState::Stopping;
                    }
                }
                ClientState::Stopping => {
                    ui.add_enabled_ui(false, |ui| {
                        let _ = ui.button("Stopping test..");
                    });
                }
                ClientState::Stopped => {
                    if ui.button("Start test").clicked() {
                        self.start_client(ctx)
                    }
                }
            }
        });

        ui.separator();

        ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add_enabled_ui(active, |ui| {
                if compact {
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.settings.client.download, "Download").on_hover_text("Run a download test");
                        ui.add_space(10.0);
                        ui.checkbox(&mut self.settings.client.upload, "Upload").on_hover_text("Run an upload test");
                        ui.add_space(10.0);
                        ui.checkbox(&mut self.settings.client.bidirectional, "Bidirectional").on_hover_text("Run a test doing both download and upload");
                    });
                    Grid::new("settings-compact").show(ui, |ui| {
                        ui.label("Streams: ").on_hover_text("The number of TCP connections used to generate traffic in a single direction");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.streams)
                                .range(1..=1000)
                                .speed(0.05),
                        );
                        ui.end_row();
                        ui.label("Load duration: ").on_hover_text("The duration in which traffic is generated");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.load_duration)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.end_row();
                        ui.label("Grace duration: ").on_hover_text("The idle time between each test");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.grace_duration)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.end_row();
                        ui.label("Stream stagger: ").on_hover_text("The delay between the start of each stream");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.stream_stagger)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.end_row();
                        ui.label("Latency sample interval:");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.settings.client.latency_sample_interval,
                            )
                            .range(1..=1000)
                            .speed(0.05),
                        );
                        ui.label("milliseconds");
                        ui.end_row();
                        ui.label("Throughput sample interval:");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.settings.client.throughput_sample_interval,
                            )
                            .range(1..=1000)
                            .speed(0.05),
                        );
                        ui.label("milliseconds");
                        ui.end_row();
                    });
                } else {
                    Grid::new("settings").show(ui, |ui| {
                        ui.checkbox(&mut self.settings.client.download, "Download")
                            .on_hover_text("Run a download test");
                        ui.allocate_space(vec2(1.0, 1.0));
                        ui.label("Streams: ").on_hover_text("The number of TCP connections used to generate traffic in a single direction");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.streams)
                                .range(1..=1000)
                                .speed(0.05),
                        );
                        ui.label("");
                        ui.allocate_space(vec2(1.0, 1.0));

                        ui.label("Stream stagger: ").on_hover_text("The delay between the start of each stream");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.stream_stagger)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.end_row();

                        ui.checkbox(&mut self.settings.client.upload, "Upload")
                            .on_hover_text("Run an upload test");
                        ui.label("");
                        ui.label("Load duration: ").on_hover_text("The duration in which traffic is generated");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.load_duration)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.label("");

                        ui.label("Latency sample interval: ");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.settings.client.latency_sample_interval,
                            )
                            .range(1..=1000)
                            .speed(0.05),
                        );
                        ui.label("milliseconds");
                        ui.end_row();

                        ui.checkbox(&mut self.settings.client.bidirectional, "Bidirectional")
                            .on_hover_text("Run a test doing both download and upload");
                        ui.label("");
                        ui.label("Grace duration: ").on_hover_text("The idle time between each test");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.client.grace_duration)
                                .range(0..=1000)
                                .speed(0.05),
                        );
                        ui.label("seconds");
                        ui.label("");
                        ui.label("Throughput sample interval: ");
                        ui.add(
                            egui::DragValue::new(
                                &mut self.settings.client.throughput_sample_interval,
                            )
                            .range(1..=1000)
                            .speed(0.05),
                        );
                        ui.label("milliseconds");
                        ui.end_row();
                    });
                }

                let parameters_changed = self.settings.client.config() != ClientSettings::default().config();

                ui.add_enabled_ui(parameters_changed, |ui| {
                    if ui.button("Reset").clicked()  {
                        let mut default = ClientSettings::default();
                        default.server = self.settings.client.server.clone();
                        default.latency_peer_server = self.settings.client.latency_peer_server.clone();
                        default.latency_peer = self.settings.client.latency_peer;
                        self.settings.client = default;
                    }
                });

                ui.separator();

                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.settings.client.latency_peer, "Latency peer:").on_hover_text("Specifies another server (peer) which will also measure the latency to the server independently of the client");
                    ui.add_enabled_ui(self.settings.client.latency_peer, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.settings.client.latency_peer_server)
                                .hint_text("(Locate local peer)"),
                        );
                    });
                });
            });

            if self.client_state == ClientState::Running
                || self.client_state == ClientState::Stopping
            {
                let client = self.client.as_mut().unwrap();

                while let Ok(msg) = client.rx.try_recv() {
                    println!("[Client] {msg}");
                    self.msgs.push(msg);
                }

                if let Ok(result) = client.done.as_mut().unwrap().try_recv() {
                    match result {
                        Some(Ok(result)) => {
                            self.msgs.push(with_time("Test complete"));
                            let result = result.to_test_result();
                            self.set_result(result);
                            if self.tab == Tab::Client {
                                self.tab = Tab::Result;
                            }
                        }
                        Some(Err(error)) => {
                            self.msgs.push(with_time(&format!("Error: {error}")));
                        }
                        None => {
                            self.msgs.push(with_time("Aborted..."));
                        }
                    }
                    self.client = None;
                    self.client_state = ClientState::Stopped;
                }
            }

            if !self.msgs.is_empty() {
                ui.separator();
            }

            for (i, msg) in self.msgs.iter().enumerate() {
                let response = ui.label(msg);
                if self.msg_scrolled <= i {
                    self.msg_scrolled = i + 1;
                    response.scroll_to_me(Some(Align::Max));
                }
            }
        });
    }
}
