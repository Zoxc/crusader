#![allow(
    clippy::field_reassign_with_default,
    clippy::option_map_unit_fn,
    clippy::type_complexity
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::sync::Arc;

use crusader_gui_lib::{Settings, Tester};
use eframe::{egui, emath::vec2, Theme};
use serde::{Deserialize, Serialize};

fn main() {
    let mut options = eframe::NativeOptions::default();
    options.follow_system_theme = false;
    options.default_theme = Theme::Light;

    // VSync causes performance issues so turn it off.
    options.vsync = false;

    crusader_lib::plot::register_fonts();

    let settings = std::env::current_exe()
        .ok()
        .map(|exe| exe.with_extension("toml"));

    eframe::run_native(
        "Crusader Network Tester",
        options,
        Box::new(move |_cc| {
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
