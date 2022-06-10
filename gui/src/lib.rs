#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{fs, mem, sync::Arc, time::Duration};

use eframe::{
    egui::{
        self,
        plot::{Legend, Line, LinkedAxisGroup, Plot, VLine, Value, Values},
        Grid, Layout, TextEdit, Ui,
    },
    emath::{vec2, Align, Vec2},
    epaint::Color32,
};
use library::{
    protocol, serve2,
    test2::{self, float_max, to_rates, Config},
};
use serde::{Deserialize, Serialize};
use tokio::sync::{
    mpsc::{self, error::TryRecvError},
    oneshot,
};

fn main() {
    let mut options = eframe::NativeOptions::default();

    // VSync causes performance issues so turn it off.
    options.vsync = false;

    let settings = std::env::current_exe()
        .ok()
        .and_then(|exe| fs::read_to_string(exe.with_extension("toml")).ok())
        .and_then(|data| toml::from_str(&data).ok())
        .and_then(|toml: toml::Value| {
            toml.get("client")
                .and_then(|table| table.as_table())
                .map(|table| {
                    let mut settings = Settings::default();

                    table
                        .get("server")
                        .and_then(|value| value.as_str())
                        .map(|value| settings.server = value.to_string());

                    table
                        .get("download")
                        .and_then(|value| value.as_bool())
                        .map(|value| settings.download = value);

                    table
                        .get("upload")
                        .and_then(|value| value.as_bool())
                        .map(|value| settings.upload = value);

                    table
                        .get("both")
                        .and_then(|value| value.as_bool())
                        .map(|value| settings.both = value);

                    table
                        .get("streams")
                        .and_then(|value| {
                            value.as_integer().and_then(|value| value.try_into().ok())
                        })
                        .map(|value| settings.streams = value);

                    table
                        .get("load_duration")
                        .and_then(|value| {
                            value.as_integer().and_then(|value| value.try_into().ok())
                        })
                        .map(|value| settings.load_duration = value);

                    table
                        .get("grace_duration")
                        .and_then(|value| {
                            value.as_integer().and_then(|value| value.try_into().ok())
                        })
                        .map(|value| settings.grace_duration = value);

                    table
                        .get("latency_sample_rate")
                        .and_then(|value| {
                            value.as_integer().and_then(|value| value.try_into().ok())
                        })
                        .map(|value| settings.latency_sample_rate = value);

                    table
                        .get("bandwidth_sample_rate")
                        .and_then(|value| {
                            value.as_integer().and_then(|value| value.try_into().ok())
                        })
                        .map(|value| settings.bandwidth_sample_rate = value);

                    settings
                })
        })
        .unwrap_or_default();
    eframe::run_native(
        "Bandwidth and latency tester",
        options,
        Box::new(|_cc| {
            Box::new(Tester {
                tab: Tab::Client,
                settings,
                client_state: ClientState::Stopped,
                client: None,
                result: None,
                msgs: Vec::new(),
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
    done: Option<oneshot::Receiver<Option<Result<TestResult, String>>>>,
    abort: Option<oneshot::Sender<()>>,
}

#[derive(PartialEq, Eq)]
enum ClientState {
    Stopped,
    Stopping,
    Running,
}

#[derive(PartialEq, Eq)]
enum Tab {
    Client,
    Server,
    Result,
}

#[derive(Serialize, Deserialize)]
struct TomlSettings {
    client: Option<Settings>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    server: String,
    download: bool,
    upload: bool,
    both: bool,
    streams: u64,
    load_duration: u64,
    grace_duration: u64,
    latency_sample_rate: u64,
    bandwidth_sample_rate: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: "localhost".to_owned(),
            download: true,
            upload: true,
            both: true,
            streams: 16,
            load_duration: 5,
            grace_duration: 1,
            latency_sample_rate: 5,
            bandwidth_sample_rate: 20,
        }
    }
}

struct Tester {
    tab: Tab,
    settings: Settings,
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
    latency_max: f64,
    bandwidth_max: f64,
}

pub fn handle_bytes(data: &[(u64, f64)], start: f64) -> Vec<(f64, f64)> {
    to_rates(data)
        .into_iter()
        .map(|(time, speed)| (Duration::from_micros(time).as_secs_f64() - start, speed))
        .collect()
}

impl Drop for Tester {
    fn drop(&mut self) {
        // Stop client
        self.client.as_mut().map(|client| {
            mem::take(&mut client.abort).map(|abort| {
                abort.send(()).unwrap();
            });
            mem::take(&mut client.done).map(|done| {
                done.blocking_recv().ok();
            });
        });

        // Stop server
        self.server.as_mut().map(|server| {
            mem::take(&mut server.stop).map(|stop| {
                stop.send(()).unwrap();
            });
            mem::take(&mut server.done).map(|done| {
                done.blocking_recv().ok();
            });
        });

        // Store settings
        toml::ser::to_string_pretty(&TomlSettings {
            client: Some(self.settings.clone()),
        })
        .map(|data| {
            std::env::current_exe()
                .map(|exe| fs::write(exe.with_extension("toml"), data.as_bytes()))
        })
        .ok();
    }
}

impl Tester {
    fn start_client(&mut self, ctx: &egui::Context) {
        self.client_state = ClientState::Running;
        self.msgs.clear();

        let config = Config {
            port: protocol::PORT,
            streams: self.settings.streams,
            grace_duration: self.settings.grace_duration,
            load_duration: self.settings.load_duration,
            download: self.settings.download,
            upload: self.settings.upload,
            both: self.settings.both,
            ping_interval: self.settings.latency_sample_rate,
            bandwidth_interval: self.settings.bandwidth_sample_rate,
            plot_transferred: false,
            plot_width: None,
            plot_height: None,
        };

        let (signal_done, done) = oneshot::channel();
        let (tx, rx) = mpsc::unbounded_channel();

        let ctx = ctx.clone();
        let ctx_ = ctx.clone();

        let abort = test2::test_callback(
            config,
            &self.settings.server,
            Arc::new(move |msg| {
                tx.send(msg.to_string()).unwrap();
                ctx.request_repaint();
            }),
            Box::new(move |result| {
                signal_done
                    .send(result.map(|result| {
                        result.map(|result| {
                            let start = result.start.as_secs_f64();
                            let download = handle_bytes(&result.combined_download_bytes, start);
                            let upload = handle_bytes(&result.combined_upload_bytes, start);
                            let both =
                                handle_bytes(result.both_bytes.as_deref().unwrap_or(&[]), start);
                            let latency: Vec<_> = result
                                .pings
                                .iter()
                                .filter(|v| v.1 >= result.start)
                                .filter_map(|(_, sent, latency)| {
                                    latency.map(|latency| {
                                        (sent.as_secs_f64() - start, latency.as_secs_f64() * 1000.0)
                                    })
                                })
                                .collect();
                            let loss = result
                                .pings
                                .iter()
                                .filter(|v| v.1 >= result.start)
                                .filter_map(|(_, sent, latency)| {
                                    if latency.is_none() {
                                        Some(sent.as_secs_f64() - start)
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            let download_max = float_max(download.iter().map(|v| v.1));
                            let upload_max = float_max(upload.iter().map(|v| v.1));
                            let both_max = float_max(both.iter().map(|v| v.1));
                            let bandwidth_max =
                                float_max([download_max, upload_max, both_max].into_iter());
                            let latency_max = float_max(latency.iter().map(|v| v.1));
                            TestResult {
                                result,
                                download,
                                upload,
                                both,
                                latency,
                                loss,
                                bandwidth_max,
                                latency_max,
                            }
                        })
                    }))
                    .map_err(|_| ())
                    .unwrap();
                ctx_.request_repaint();
            }),
        );

        self.client = Some(Client {
            done: Some(done),
            rx,
            abort: Some(abort),
        });
        self.client_state = ClientState::Running;
        self.result = None;
        self.result_saved = None;
    }

    fn client(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let active = self.client_state == ClientState::Stopped;

        ui.horizontal(|ui| {
            ui.add_enabled_ui(active, |ui| {
                ui.label("Server address:");
                ui.add(TextEdit::singleline(&mut self.settings.server));
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

        ui.add_enabled_ui(active, |ui| {
            Grid::new("settings").show(ui, |ui| {
                ui.checkbox(&mut self.settings.download, "Download");
                ui.allocate_space(vec2(1.0, 1.0));
                ui.label("Streams: ");
                ui.add(
                    egui::DragValue::new(&mut self.settings.streams)
                        .clamp_range(1..=1000)
                        .speed(0.05),
                );
                ui.label("");
                ui.allocate_space(vec2(1.0, 1.0));
                ui.label("Latency sample rate");
                ui.add(
                    egui::DragValue::new(&mut self.settings.latency_sample_rate)
                        .clamp_range(1..=1000)
                        .speed(0.05),
                );
                ui.label("milliseconds");
                ui.end_row();

                ui.checkbox(&mut self.settings.upload, "Upload");
                ui.label("");
                ui.label("Load duration: ");
                ui.add(
                    egui::DragValue::new(&mut self.settings.load_duration)
                        .clamp_range(1..=1000)
                        .speed(0.05),
                );
                ui.label("seconds");
                ui.label("");
                ui.label("Bandwidth sample rate");
                ui.add(
                    egui::DragValue::new(&mut self.settings.bandwidth_sample_rate)
                        .clamp_range(1..=1000)
                        .speed(0.05),
                );
                ui.label("milliseconds");
                ui.end_row();

                ui.checkbox(&mut self.settings.both, "Both");
                ui.label("");
                ui.label("Grace duration: ");
                ui.add(
                    egui::DragValue::new(&mut self.settings.grace_duration)
                        .clamp_range(1..=1000)
                        .speed(0.05),
                );
                ui.label("seconds");
                ui.end_row();
            });
        });

        if self.client_state == ClientState::Running || self.client_state == ClientState::Stopping {
            let client = self.client.as_mut().unwrap();

            while let Ok(msg) = client.rx.try_recv() {
                println!("[Client] {msg}");
                self.msgs.push(msg);
            }

            if let Ok(result) = client.done.as_mut().unwrap().try_recv() {
                match result {
                    Some(Ok(result)) => {
                        self.msgs.push("Test complete.".to_owned());
                        self.result = Some(result);
                        if self.tab == Tab::Client {
                            self.tab = Tab::Result;
                        }
                    }
                    Some(Err(error)) => {
                        self.msgs.push(format!("Error: {error}"));
                    }
                    None => {
                        self.msgs.push("Aborted...".to_owned());
                    }
                }
                self.client = None;
                self.client_state = ClientState::Stopped;
            }
        }

        if !self.msgs.is_empty() {
            ui.separator();
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

        ui.allocate_space(vec2(1.0, 15.0));

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let duration = result.result.duration.as_secs_f64() * 1.1;

            // Packet loss
            let plot = Plot::new("loss")
                .legend(Legend::default())
                .show_axes([false, false])
                .link_axis(self.axis.clone())
                .include_x(0.0)
                .include_x(duration)
                .include_y(0.0)
                .include_y(1.0)
                .height(20.0)
                .label_formatter(|_, value| format!("Time = {:.2} s", value.x));

            plot.show(ui, |plot_ui| {
                for loss in &result.loss {
                    plot_ui.vline(VLine::new(*loss).color(Color32::from_rgb(193, 85, 85)))
                }
            });
            ui.label("Packet loss");

            // Latency

            let plot = Plot::new("ping")
                .legend(Legend::default())
                .height(ui.available_height() / 2.0)
                .link_axis(self.axis.clone())
                .include_x(0.0)
                .include_x(duration)
                .include_y(0.0)
                .include_y(result.latency_max * 1.1)
                .label_formatter(|_, value| {
                    format!("Latency = {:.2} ms\nTime = {:.2} s", value.y, value.x)
                });

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
                .set_margin_fraction(Vec2 { x: 0.2, y: 0.0 })
                .include_x(0.0)
                .include_x(duration)
                .include_y(0.0)
                .include_y(result.bandwidth_max * 1.1)
                .label_formatter(|_, value| {
                    format!("Bandwidth = {:.2} Mbps\nTime = {:.2} s", value.y, value.x)
                });

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
                        self.server = None;
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
        let mut style = ctx.style().clone();
        let style_ = Arc::make_mut(&mut style);
        style_.spacing.button_padding = vec2(6.0, 0.0);
        style_.spacing.interact_size.y = 30.0;
        style_.spacing.item_spacing = vec2(5.0, 5.0);
        ctx.set_style(style);

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
