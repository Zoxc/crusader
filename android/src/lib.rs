#![cfg(target_os = "android")]

#[ndk_glue::main(backtrace = "on")]
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
                test: "".to_owned(),
            })
        }),
    );
}

use std::{
    error::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use eframe::{
    egui::{self, Layout},
    emath::vec2,
    epaint::FontFamily,
    Renderer,
};
use jni::objects::{JObject, JValue};

struct AndroidApp {
    tester: gui::Tester,
    test: String,
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

                show_keyboard(ctx.wants_keyboard_input()).unwrap();
            });
        });
    }
}

fn show_keyboard(show: bool) -> Result<(), Box<dyn Error>> {
    let context = ndk_context::android_context();

    let vm = unsafe { jni::JavaVM::from_raw(context.vm().cast())? };

    let activity: JObject = (context.context() as jni::sys::jobject).into();

    let env = vm.attach_current_thread()?;

    let input_method = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[env.new_string("input_method")?.into()],
        )?
        .l()?;

    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?
        .l()?;

    let view = env
        .call_method(window, "getDecorView", "()Landroid/view/View;", &[])?
        .l()?;
    /*
    let view = env
        .call_method(view, "getRootView", "()Landroid/view/View;", &[])?
        .l()?;


       let view = env
           .call_method(activity, "getCurrentFocus", "()Landroid/view/View;", &[])?
           .l()?;
    */
    if show {
        env.call_method(
            input_method,
            "showSoftInput",
            "(Landroid/view/View;I)Z",
            &[view.into(), JValue::Int(0)],
        )
        .unwrap();
    } else {
        let token = env
            .call_method(view, "getWindowToken", "()Landroid/os/IBinder;", &[])
            .unwrap()
            .l()?;
        assert!(!token.into_inner().is_null());
        env.call_method(
            input_method,
            "hideSoftInput",
            "(Landroid/os/IBinder;I)Z",
            &[token.into(), JValue::Int(0)],
        )
        .unwrap();
    }

    Ok(())
}
