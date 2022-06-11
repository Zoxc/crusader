#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::sync::Arc;

use eframe::{
    egui::{self},
    emath::vec2,
};

fn main() {
    let mut options = eframe::NativeOptions::default();

    // VSync causes performance issues so turn it off.
    options.vsync = false;

    let settings = std::env::current_exe()
        .ok()
        .map(|exe| gui::load_settings(&exe.with_extension("toml")))
        .unwrap_or_default();

    eframe::run_native(
        "Bandwidth and latency tester",
        options,
        Box::new(|_cc| {
            Box::new(App {
                tester: gui::Tester::new(settings),
            })
        }),
    );
}

struct App {
    tester: gui::Tester,
}

impl Drop for App {
    fn drop(&mut self) {}
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut style = ctx.style().clone();
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
