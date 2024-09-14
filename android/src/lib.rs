#![allow(
    clippy::field_reassign_with_default,
    clippy::option_map_unit_fn,
    clippy::missing_safety_doc
)]

use crusader_gui_lib::Tester;
use crusader_lib::file_format::RawResult;
use eframe::egui::{self, vec2, Align, FontFamily, Layout};
use jni::{
    objects::{JClass, JObject, JString},
    sys::{jboolean, jbyteArray},
    JNIEnv,
};
use std::{
    error::Error,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex},
};

#[cfg(target_os = "android")]
use {
    android_activity::AndroidApp,
    crusader_lib::test::PlotConfig,
    eframe::{NativeOptions, Renderer, Theme},
    log::Level,
    std::fs,
    winit::platform::android::EventLoopBuilderExtAndroid,
};

struct App {
    tester: Tester,
    keyboard_shown: bool,
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
            let mut ui = ui.child_ui(rect, Layout::left_to_right(Align::Center), None);
            ui.vertical(|ui| {
                ui.heading("Crusader Network Benchmark");
                ui.separator();

                SAVED_FILE.lock().unwrap().take().map(|(image, name)| {
                    if !image {
                        self.tester.save_raw(Path::new(&name).to_owned());
                    }
                });

                LOADED_FILE.lock().unwrap().take().map(|(name, data)| {
                    RawResult::load_from_reader(Cursor::new(data))
                        .map(|data| self.tester.load_file(Path::new(&name).to_owned(), data));
                });

                self.tester.show(ctx, ui);
            });
        });

        if ctx.wants_keyboard_input() != self.keyboard_shown {
            show_keyboard(ctx.wants_keyboard_input()).unwrap();
            self.keyboard_shown = ctx.wants_keyboard_input();
        }
    }
}

fn show_keyboard(show: bool) -> Result<(), Box<dyn Error>> {
    let context = ndk_context::android_context();

    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast())? };

    let activity: JObject = (context.context() as jni::sys::jobject).into();

    let env = vm.attach_current_thread()?;

    env.call_method(activity, "showKeyboard", "(Z)V", &[show.into()])?
        .v()?;
    Ok(())
}

fn save_file(image: bool, name: String, data: Vec<u8>) -> Result<(), Box<dyn Error>> {
    let context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast())? };
    let activity: JObject = (context.context() as jni::sys::jobject).into();
    let env = vm.attach_current_thread()?;
    env.call_method(
        activity,
        "saveFile",
        "(ZLjava/lang/String;[B)V",
        &[
            image.into(),
            env.new_string(name).unwrap().into(),
            env.byte_array_from_slice(&data).unwrap().into(),
        ],
    )?
    .v()?;
    Ok(())
}

static SAVED_FILE: Mutex<Option<(bool, String)>> = Mutex::new(None);

#[no_mangle]
pub unsafe extern "C" fn Java_zoxc_crusader_MainActivity_fileSaved(
    env: JNIEnv,
    _: JClass,
    image: jboolean,
    name: JString,
) {
    let name: String = env.get_string(name).unwrap().into();
    *SAVED_FILE.lock().unwrap() = Some((image != 0, name));
}

fn load_file() -> Result<(), Box<dyn Error>> {
    let context = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast())? };
    let activity: JObject = (context.context() as jni::sys::jobject).into();
    let env = vm.attach_current_thread()?;
    env.call_method(activity, "loadFile", "()V", &[])?.v()?;
    Ok(())
}

static LOADED_FILE: Mutex<Option<(String, Vec<u8>)>> = Mutex::new(None);

#[no_mangle]
pub unsafe extern "C" fn Java_zoxc_crusader_MainActivity_fileLoaded(
    env: JNIEnv,
    _: JClass,
    name: JString,
    data: jbyteArray,
) {
    let name: String = env.get_string(name).unwrap().into();
    let data = env.convert_byte_array(data).unwrap();
    *LOADED_FILE.lock().unwrap() = Some((name, data));
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_min_level(Level::Trace));

    crusader_lib::plot::register_fonts();

    let settings = app
        .internal_data_path()
        .map(|path| path.join("settings.toml"));

    let temp_plot = app.internal_data_path().map(|path| path.join("plot.png"));

    let mut options = NativeOptions::default();
    options.follow_system_theme = false;
    options.default_theme = Theme::Light;
    options.renderer = Renderer::Wgpu;
    options.event_loop_builder = Some(Box::new(move |builder| {
        builder.with_android_app(app.clone());
    }));
    let mut tester = Tester::new(settings);
    tester.file_loader = Some(Box::new(|_| load_file().unwrap()));
    tester.plot_saver = Some(Box::new(move |result| {
        let path = temp_plot.as_deref().unwrap();
        crusader_lib::plot::save_graph_to_path(path, &PlotConfig::default(), result).unwrap();
        let data = fs::read(path).unwrap();
        fs::remove_file(path).unwrap();
        let name = format!("{}.png", crusader_lib::test::timed("plot"));
        save_file(true, name, data).unwrap();
    }));
    tester.raw_saver = Some(Box::new(|result| {
        let mut writer = Cursor::new(Vec::new());
        result.save_to_writer(&mut writer).unwrap();
        let data = writer.into_inner();
        let name = format!("{}.crr", crusader_lib::test::timed("data"));
        save_file(false, name, data).unwrap();
    }));
    eframe::run_native(
        "Crusader Network Tester",
        options,
        Box::new(|_cc| {
            Ok(Box::new(App {
                tester,
                keyboard_shown: false,
            }))
        }),
    )
    .unwrap();
}
