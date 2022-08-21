#![allow(
    clippy::field_reassign_with_default,
    clippy::option_map_unit_fn,
    clippy::type_complexity
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{fs, sync::Arc};

use crusader_gui_lib::{Settings, Tester};
use eframe::{egui, emath::vec2};
use serde::{Deserialize, Serialize};

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
        "Crusader Network Tester",
        options,
        Box::new(|_cc| {
            Box::new(App {
                tester: Tester::new(settings),
            })
        }),
    );
}

#[derive(Serialize, Deserialize)]
struct TomlSettings {
    client: Option<Settings>,
}

struct App {
    tester: Tester,
}

impl Drop for App {
    fn drop(&mut self) {}
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut style = ctx.style();
        let style_ = Arc::make_mut(&mut style);
        style_.spacing.button_padding = vec2(6.0, 0.0);
        style_.spacing.interact_size.y = 30.0;
        style_.spacing.item_spacing = vec2(5.0, 5.0);
        ctx.set_style(style);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.tester.show(ctx, ui);
        });
    }
}
