#![allow(
    clippy::field_reassign_with_default,
    clippy::option_map_unit_fn,
    clippy::type_complexity
)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{error::Error, sync::Arc};

use crusader_gui_lib::Tester;
use crusader_lib::LIB_VERSION;
use eframe::{
    egui::{self, Context, FontData, FontDefinitions, FontFamily},
    emath::vec2,
    Theme,
};
#[allow(unused_imports)]
use font_kit::family_name::FamilyName;
use font_kit::{handle::Handle, properties::Properties, source::SystemSource};

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
        &format!("Crusader Network Tester {}", LIB_VERSION),
        options,
        Box::new(move |cc| {
            let ctx = &cc.egui_ctx;
            let mut style = ctx.style();
            let style_ = Arc::make_mut(&mut style);

            style_.spacing.button_padding = vec2(6.0, 0.0);
            style_.spacing.interact_size.y = 30.0;
            style_.spacing.item_spacing = vec2(5.0, 5.0);

            let font_size = if cfg!(target_os = "macos") {
                13.5
            } else {
                12.5
            };

            style_.text_styles.get_mut(&egui::TextStyle::Body).map(|v| {
                v.size = font_size;
            });
            style_
                .text_styles
                .get_mut(&egui::TextStyle::Button)
                .map(|v| {
                    v.size = font_size;
                });
            style_
                .text_styles
                .get_mut(&egui::TextStyle::Small)
                .map(|v| {
                    v.size = font_size;
                });
            style_
                .text_styles
                .get_mut(&egui::TextStyle::Monospace)
                .map(|v| {
                    v.size = font_size;
                });
            ctx.set_style(style);

            load_system_font(ctx).ok();

            Ok(Box::new(App {
                tester: Tester::new(settings),
            }))
        }),
    )
    .unwrap()
}

fn load_system_font(ctx: &Context) -> Result<(), Box<dyn Error>> {
    let mut fonts = FontDefinitions::default();

    let handle = SystemSource::new().select_best_match(
        &[
            #[cfg(target_os = "macos")]
            FamilyName::SansSerif,
            #[cfg(windows)]
            FamilyName::Title("Segoe UI".to_string()),
        ],
        &Properties::new(),
    )?;

    let buf: Vec<u8> = match handle {
        Handle::Memory { bytes, .. } => bytes.to_vec(),
        Handle::Path { path, .. } => std::fs::read(path)?,
    };

    const UI_FONT: &str = "System Sans Serif";

    fonts
        .font_data
        .insert(UI_FONT.to_owned(), FontData::from_owned(buf));

    if let Some(vec) = fonts.families.get_mut(&FontFamily::Proportional) {
        vec.insert(0, UI_FONT.to_owned());
    }

    if let Some(vec) = fonts.families.get_mut(&FontFamily::Monospace) {
        vec.insert(0, UI_FONT.to_owned());
    }

    ctx.set_fonts(fonts);

    Ok(())
}

struct App {
    tester: Tester,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.tester.show(ctx, ui);
        });
    }
}
