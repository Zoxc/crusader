#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    println!("-----------Start --------");
    println!("-----------Start --------");
    println!("-----------Start --------");
    let mut options = eframe::NativeOptions::default();
    options.vsync = false;
    options.renderer = Renderer::Wgpu;
    eframe::run_native(
        "My egui App",
        options,
        Box::new(|_cc| {
            Box::new(AndroidApp {
                tester: gui::Tester::new(Default::default()),
            })
        }),
    );
}

use std::sync::Arc;

use eframe::{
    egui::{self, Layout},
    emath::vec2,
    epaint::FontFamily,
    Renderer,
};

struct AndroidApp {
    tester: gui::Tester,
}

impl eframe::App for AndroidApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use crate::egui::FontFamily::Proportional;
        use crate::egui::FontId;
        use crate::egui::TextStyle::*;

        let mut style = ctx.style().clone();
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
            let mut ui = ui.child_ui(rect, Layout::left_to_right());
            ui.vertical(|ui| {
                ui.heading("Crusader Network Benchmark");
                ui.separator();
                self.tester.show(ctx, ui);

                ui.label(if ctx.wants_keyboard_input() {
                    ndk_glue::native_activity().show_soft_input(false);
                    "Keyboard"
                } else {
                    ndk_glue::native_activity().hide_soft_input(false);
                    "No keyboard"
                });
            });
        });
    }
}
