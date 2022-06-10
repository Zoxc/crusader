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
        Box::new(|_cc| Box::new(Tester {})),
    );
}

struct Tester {}

impl Drop for Tester {
    fn drop(&mut self) {}
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
            ui.label("App!");
        });
    }
}
