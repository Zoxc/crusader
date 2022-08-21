#![allow(clippy::field_reassign_with_default)]

use crusader_gui_lib::{Settings, Tester};
use eframe::egui::{self, vec2, Align, FontFamily, Layout};
use std::sync::Arc;

#[cfg(target_os = "android")]
use {
    android_activity::AndroidApp,
    eframe::{NativeOptions, Renderer},
    log::Level,
};

struct App {
    tester: Tester,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use eframe::egui::FontFamily::Proportional;
        use eframe::egui::FontId;
        use eframe::egui::TextStyle::*;

        let mut style = ctx.style();
        let style_ = Arc::make_mut(&mut style);
        style_.spacing.button_padding = vec2(10.0, 0.0);
        style_.spacing.interact_size.y = 40.0;
        style_.spacing.item_spacing = vec2(10.0, 10.0);

        style_.text_styles = [
            (Heading, FontId::new(26.0, Proportional)),
            (Body, FontId::new(16.0, Proportional)),
            (Monospace, FontId::new(16.0, FontFamily::Monospace)),
            (Button, FontId::new(16.0, Proportional)),
            (Small, FontId::new(16.0, Proportional)),
        ]
        .into();

        ctx.set_style(style);

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut rect = ui.max_rect();
            rect.set_top(rect.top() + 40.0);
            rect.set_height(rect.height() - 60.0);
            let mut ui = ui.child_ui(rect, Layout::left_to_right(Align::Center));
            ui.vertical(|ui| {
                ui.heading("Crusader Network Benchmark");
                ui.separator();

                self.tester.show(ctx, ui);
            });
        });
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_min_level(Level::Trace));

    crusader_lib::plot::register_fonts();

    let mut options = NativeOptions::default();
    options.android_app = Some(app);
    options.renderer = Renderer::Wgpu;
    eframe::run_native(
        "Crusader Network Tester",
        options,
        Box::new(|_cc| {
            Box::new(App {
                tester: Tester::new(Settings::default()),
            })
        }),
    );
}
