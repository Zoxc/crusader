#![allow(
    clippy::field_reassign_with_default,
    clippy::option_map_unit_fn,
    clippy::type_complexity
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{
    fs, mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crusader_lib::{
    file_format::RawResult,
    plot::{self, float_max, to_rates},
    protocol, serve,
    test::{self, Config, PlotConfig},
};
use eframe::{
    egui::{
        self,
        plot::{Legend, Line, LinkedAxisGroup, LinkedCursorsGroup, Plot, PlotPoints},
        Grid, Layout, ScrollArea, TextEdit, TextStyle, Ui,
    },
    emath::{vec2, Align, Vec2},
    epaint::Color32,
};

#[cfg(not(target_os = "android"))]
use rfd::FileDialog;

use serde::{Deserialize, Serialize};
use tokio::sync::{
    mpsc::{self, error::TryRecvError},
    oneshot,
};

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
    done: Option<oneshot::Receiver<Option<Result<RawResult, String>>>>,
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
pub struct Settings {
    pub server: String,
    pub download: bool,
    pub upload: bool,
    pub both: bool,
    pub streams: u64,
    pub load_duration: u64,
    pub grace_duration: u64,
    pub latency_sample_rate: u64,
    pub bandwidth_sample_rate: u64,
}

impl Settings {
    fn from_path(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
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
            .unwrap_or_default()
    }
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

pub struct Tester {
    tab: Tab,
    settings: Settings,
    settings_path: Option<PathBuf>,
    server_state: ServerState,
    server: Option<Server>,
    client_state: ClientState,
    client: Option<Client>,
    result: Option<TestResult>,
    result_saved: Option<String>,
    raw_result: Option<RawResult>,
    raw_result_saved: Option<String>,
    msgs: Vec<String>,
    msg_scrolled: usize,
    axis: LinkedAxisGroup,
    cursor: LinkedCursorsGroup,
    pub file_loader: Option<Box<dyn Fn(&mut Tester)>>,
    pub plot_saver: Option<Box<dyn Fn(&plot::TestResult)>>,
    pub raw_saver: Option<Box<dyn Fn(&RawResult)>>,
}

pub struct TestResult {
    result: plot::TestResult,
    download: Vec<(f64, f64)>,
    upload: Vec<(f64, f64)>,
    both: Vec<(f64, f64)>,
    latency: Vec<(f64, f64)>,
    latency_max: f64,
    up_latency: Vec<(f64, f64)>,
    down_latency: Vec<(f64, f64)>,
    loss: Vec<(f64, Option<bool>)>,
    bandwidth_max: f64,
}

impl TestResult {
    fn new(result: plot::TestResult) -> Self {
        let start = result.start.as_secs_f64();
        let download = handle_bytes(&result.combined_download_bytes, start);
        let upload = handle_bytes(&result.combined_upload_bytes, start);
        let both = handle_bytes(result.both_bytes.as_deref().unwrap_or(&[]), start);

        let latency: Vec<_> = result
            .pings
            .iter()
            .filter(|p| p.sent >= result.start)
            .filter_map(|p| {
                p.latency.and_then(|latency| {
                    latency
                        .total
                        .map(|total| (p.sent.as_secs_f64() - start, total.as_secs_f64() * 1000.0))
                })
            })
            .collect();

        let up_latency: Vec<_> = result
            .pings
            .iter()
            .filter(|p| p.sent >= result.start)
            .filter_map(|p| {
                p.latency.map(|latency| {
                    (
                        p.sent.as_secs_f64() - start,
                        latency.up.as_secs_f64() * 1000.0,
                    )
                })
            })
            .collect();

        let down_latency: Vec<_> = result
            .pings
            .iter()
            .filter(|p| p.sent >= result.start)
            .filter_map(|p| {
                p.latency.and_then(|latency| {
                    latency
                        .down()
                        .map(|down| (p.sent.as_secs_f64() - start, down.as_secs_f64() * 1000.0))
                })
            })
            .collect();

        let loss = result
            .pings
            .iter()
            .filter(|p| p.sent >= result.start)
            .filter_map(|ping| {
                if ping.latency.and_then(|latency| latency.total).is_none() {
                    let down_loss =
                        (result.raw_result.version >= 2).then_some(ping.latency.is_some());
                    Some((ping.sent.as_secs_f64() - start, down_loss))
                } else {
                    None
                }
            })
            .collect();
        let download_max = float_max(download.iter().map(|v| v.1));
        let upload_max = float_max(upload.iter().map(|v| v.1));
        let both_max = float_max(both.iter().map(|v| v.1));
        let bandwidth_max = float_max([download_max, upload_max, both_max].into_iter());
        let latency_max = float_max(latency.iter().map(|v| v.1));

        TestResult {
            result,
            download,
            upload,
            both,
            latency,
            up_latency,
            down_latency,
            loss,
            bandwidth_max,
            latency_max,
        }
    }
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
        self.settings_path.as_deref().map(|path| {
            toml::ser::to_string_pretty(&TomlSettings {
                client: Some(self.settings.clone()),
            })
            .map(|data| fs::write(path, data.as_bytes()))
            .ok();
        });
    }
}

impl Tester {
    pub fn new(settings_path: Option<PathBuf>) -> Tester {
        Tester {
            tab: Tab::Client,
            settings: settings_path
                .as_deref()
                .map_or(Settings::default(), Settings::from_path),
            settings_path,
            client_state: ClientState::Stopped,
            client: None,
            result: None,
            result_saved: None,
            raw_result: None,
            raw_result_saved: None,
            msgs: Vec::new(),
            msg_scrolled: 0,
            server_state: ServerState::Stopped(None),
            server: None,
            axis: LinkedAxisGroup::x(),
            cursor: LinkedCursorsGroup::x(),
            file_loader: None,
            raw_saver: None,
            plot_saver: None,
        }
    }

    pub fn load_file(&mut self, name: String, raw: RawResult) {
        let result = raw.to_test_result();
        self.result = Some(TestResult::new(result));
        self.result_saved = None;
        self.raw_result = Some(raw);
        self.raw_result_saved = Some(name);
    }

    pub fn save_raw(&mut self, name: String) {
        self.raw_result_saved = Some(name);
    }

    pub fn save_plot(&mut self, name: String) {
        self.result_saved = Some(name);
    }

    fn config(&self) -> Config {
        Config {
            port: protocol::PORT,
            stream_stagger: Duration::from_secs(0),
            streams: self.settings.streams,
            grace_duration: Duration::from_secs(self.settings.grace_duration),
            load_duration: Duration::from_secs(self.settings.load_duration),
            download: self.settings.download,
            upload: self.settings.upload,
            both: self.settings.both,
            ping_interval: Duration::from_millis(self.settings.latency_sample_rate),
            bandwidth_interval: Duration::from_millis(self.settings.bandwidth_sample_rate),
        }
    }

    fn start_client(&mut self, ctx: &egui::Context) {
        self.client_state = ClientState::Running;
        self.msgs.clear();
        self.msg_scrolled = 0;

        let (signal_done, done) = oneshot::channel();
        let (tx, rx) = mpsc::unbounded_channel();

        let ctx = ctx.clone();
        let ctx_ = ctx.clone();

        let abort = test::test_callback(
            self.config(),
            &self.settings.server,
            Arc::new(move |msg| {
                tx.send(msg.to_string()).unwrap();
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
        self.result = None;
        self.result_saved = None;
        self.raw_result = None;
        self.raw_result_saved = None;
    }

    fn load_result(&mut self) {
        #[cfg(not(target_os = "android"))]
        {
            FileDialog::new()
                .add_filter("Crusader Raw Result", &["crr"])
                .add_filter("All files", &["*"])
                .pick_file()
                .map(|file| {
                    RawResult::load(&file).map(|raw| {
                        self.load_file(
                            file.file_name()
                                .unwrap_or_default()
                                .to_str()
                                .unwrap_or_default()
                                .to_string(),
                            raw,
                        );
                    })
                });
        }
        let file_loader = self.file_loader.take();
        file_loader.as_ref().map(|loader| loader(self));
        self.file_loader = file_loader;
    }

    fn client(&mut self, ctx: &egui::Context, ui: &mut Ui, compact: bool) {
        let active = self.client_state == ClientState::Stopped;

        ui.horizontal_wrapped(|ui| {
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

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.add_enabled_ui(active, |ui| {
                    if compact {
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.settings.download, "Download");
                            ui.add_space(10.0);
                            ui.checkbox(&mut self.settings.upload, "Upload");
                            ui.add_space(10.0);
                            ui.checkbox(&mut self.settings.both, "Both");
                        });
                        Grid::new("settings-compact").show(ui, |ui| {
                            ui.label("Streams: ");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.streams)
                                    .clamp_range(1..=1000)
                                    .speed(0.05),
                            );
                            ui.end_row();
                            ui.label("Load duration: ");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.load_duration)
                                    .clamp_range(1..=1000)
                                    .speed(0.05),
                            );
                            ui.label("seconds");
                            ui.end_row();
                            ui.label("Grace duration: ");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.grace_duration)
                                    .clamp_range(1..=1000)
                                    .speed(0.05),
                            );
                            ui.label("seconds");
                            ui.end_row();
                            ui.label("Latency sample rate:");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.latency_sample_rate)
                                    .clamp_range(1..=1000)
                                    .speed(0.05),
                            );
                            ui.label("milliseconds");
                            ui.end_row();
                            ui.label("Bandwidth sample rate:");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.bandwidth_sample_rate)
                                    .clamp_range(1..=1000)
                                    .speed(0.05),
                            );
                            ui.label("milliseconds");
                            ui.end_row();
                        });
                    } else {
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
                            ui.label("Latency sample rate:");
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
                            ui.label("Bandwidth sample rate:");
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
                    }
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
                                self.msgs.push("Test complete.".to_owned());
                                self.result = Some(TestResult::new(result.to_test_result()));
                                self.raw_result = Some(result);
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

                for (i, msg) in self.msgs.iter().enumerate() {
                    let response = ui.label(msg);
                    if self.msg_scrolled <= i {
                        self.msg_scrolled = i + 1;
                        response.scroll_to_me(Some(Align::Max));
                    }
                }
            });
    }

    fn result(&mut self, _ctx: &egui::Context, ui: &mut Ui) {
        if self.result.is_none() {
            if ui.button("Load raw data").clicked() {
                self.load_result();
            }
            ui.separator();
            ui.label("No result.");
            return;
        }

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(self.result_saved.is_none(), |ui| {
                if ui.button("Save as image").clicked() {
                    match self.plot_saver.as_ref() {
                        Some(saver) => {
                            saver(&self.result.as_ref().unwrap().result);
                        }
                        None => {
                            self.result_saved = Some(plot::save_graph(
                                &PlotConfig::default(),
                                &self.result.as_ref().unwrap().result,
                                "plot",
                            ));
                        }
                    }
                }
            });
            self.result_saved
                .as_ref()
                .map(|file| ui.label(format!("Saved as: {file}")));

            ui.add_enabled_ui(self.raw_result_saved.is_none(), |ui| {
                if ui.button("Save raw data").clicked() {
                    match self.raw_saver.as_ref() {
                        Some(saver) => {
                            saver(self.raw_result.as_ref().unwrap());
                        }
                        None => {
                            self.raw_result_saved =
                                Some(test::save_raw(self.raw_result.as_ref().unwrap(), "data"));
                        }
                    }
                }
            });
            self.raw_result_saved
                .as_ref()
                .map(|file| ui.label(format!("Saved as: {file}")));

            if ui.button("Load raw data").clicked() {
                self.load_result();
            }
        });
        ui.separator();

        let result = self.result.as_ref().unwrap();

        if result.result.raw_result.server_overload {
            ui.label("Warning: Server overload detected during test. Result should be discarded.");
            ui.separator();
        }

        if result.result.raw_result.load_termination_timeout {
            ui.label("Warning: Load termination timed out. There may be residual untracked traffic in the background.");
            ui.separator();
        }

        ui.allocate_space(vec2(1.0, 15.0));

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let duration = result.result.duration.as_secs_f64() * 1.1;

            // Packet loss
            let plot = Plot::new("loss")
                .legend(Legend::default())
                .show_axes([false, false])
                .link_axis(self.axis.clone())
                .link_cursor(self.cursor.clone())
                .center_y_axis(true)
                .include_x(0.0)
                .include_x(duration)
                .include_y(-1.0)
                .include_y(1.0)
                .height(30.0)
                .label_formatter(|_, value| format!("Time = {:.2} s", value.x));

            plot.show(ui, |plot_ui| {
                for &(loss, down_loss) in &result.loss {
                    let (color, s, e) = down_loss
                        .map(|down_loss| {
                            if down_loss {
                                (Color32::from_rgb(95, 145, 62), 1.0, 0.0)
                            } else {
                                (Color32::from_rgb(37, 83, 169), -1.0, 0.0)
                            }
                        })
                        .unwrap_or((Color32::from_rgb(193, 85, 85), -1.0, 1.0));

                    plot_ui.line(
                        Line::new(PlotPoints::from_iter(
                            [[loss, s], [loss, e]].iter().copied(),
                        ))
                        .color(color),
                    );

                    if down_loss.is_some() {
                        plot_ui.line(
                            Line::new(PlotPoints::from_iter(
                                [[loss, s], [loss, s - s / 5.0]].iter().copied(),
                            ))
                            .width(3.0)
                            .color(color),
                        );
                    }
                }
            });
            ui.label("Packet loss");

            // Latency

            let plot = Plot::new("ping")
                .legend(Legend::default())
                .height(ui.available_height() / 2.0)
                .link_axis(self.axis.clone())
                .link_cursor(self.cursor.clone())
                .include_x(0.0)
                .include_x(duration)
                .include_y(0.0)
                .include_y(result.latency_max * 1.1)
                .label_formatter(|_, value| {
                    format!("Latency = {:.2} ms\nTime = {:.2} s", value.y, value.x)
                });

            plot.show(ui, |plot_ui| {
                if result.result.raw_result.version >= 1 {
                    let latency = result.up_latency.iter().map(|v| [v.0 as f64, v.1]);
                    let latency = Line::new(PlotPoints::from_iter(latency))
                        .color(Color32::from_rgb(37, 83, 169))
                        .name("Up");

                    plot_ui.line(latency);

                    let latency = result.down_latency.iter().map(|v| [v.0 as f64, v.1]);
                    let latency = Line::new(PlotPoints::from_iter(latency))
                        .color(Color32::from_rgb(95, 145, 62))
                        .name("Down");

                    plot_ui.line(latency);
                }

                let latency = result.latency.iter().map(|v| [v.0 as f64, v.1]);
                let latency = Line::new(PlotPoints::from_iter(latency))
                    .color(Color32::from_rgb(50, 50, 50))
                    .name("Total");

                plot_ui.line(latency);
            });
            ui.label("Latency");

            // Bandwidth
            let plot = Plot::new("result")
                .legend(Legend::default())
                .link_axis(self.axis.clone())
                .link_cursor(self.cursor.clone())
                .set_margin_fraction(Vec2 { x: 0.2, y: 0.0 })
                .include_x(0.0)
                .include_x(duration)
                .include_y(0.0)
                .include_y(result.bandwidth_max * 1.1)
                .label_formatter(|_, value| {
                    format!("Bandwidth = {:.2} Mbps\nTime = {:.2} s", value.y, value.x)
                });

            plot.show(ui, |plot_ui| {
                if result.result.raw_result.download() {
                    let download = result.download.iter().map(|v| [v.0 as f64, v.1]);
                    let download = Line::new(PlotPoints::from_iter(download))
                        .color(Color32::from_rgb(95, 145, 62))
                        .name("Download");

                    plot_ui.line(download);
                }
                if result.result.raw_result.upload() {
                    let upload = result.upload.iter().map(|v| [v.0 as f64, v.1]);
                    let upload = Line::new(PlotPoints::from_iter(upload))
                        .color(Color32::from_rgb(37, 83, 169))
                        .name("Upload");

                    plot_ui.line(upload);
                }
                if result.result.raw_result.both() {
                    let both = result.both.iter().map(|v| [v.0 as f64, v.1]);
                    let both = Line::new(PlotPoints::from_iter(both))
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

                    let stop = serve::serve_until(
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

                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink([false; 2])
                    .show_rows(
                        ui,
                        ui.text_style_height(&TextStyle::Body),
                        server.msgs.len(),
                        |ui, rows| {
                            for row in rows {
                                ui.label(&server.msgs[row]);
                            }
                        },
                    );

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

    pub fn show(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let compact = ui.available_width() < 620.0;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Client, "Client");
            ui.selectable_value(&mut self.tab, Tab::Server, "Server");
            ui.selectable_value(&mut self.tab, Tab::Result, "Result");
        });
        ui.separator();

        match self.tab {
            Tab::Client => self.client(ctx, ui, compact),
            Tab::Server => self.server(ctx, ui),
            Tab::Result => self.result(ctx, ui),
        }
    }
}
