/*
#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    println!("Setup");
    unsafe {}
}
 */

#[cfg_attr(target_os = "android", ndk_glue::main(backtrace = "on"))]
pub fn main() {
    println!("-----------Start --------");
    println!("-----------Start --------");
    println!("-----------Start --------");
    let mut options = eframe::NativeOptions::default();
    options.vsync = false;
    eframe::run_native(
        "My egui App",
        options,
        Box::new(|_cc| Box::new(AndroidApp::default())),
    );
}

use eframe::egui;

struct AndroidApp {
    tester: gui::Tester,
    s: u64,
}

impl Default for AndroidApp {
    fn default() -> Self {
        Self { s: 0 }
    }
}

impl eframe::App for AndroidApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            //  println!("point - {:?}", ctx.input().pointer);
            //  println!("f{}", self.s);
            self.s += 1;
            ui.heading("My egui Application");
            ui.vertical(|ui| {
                ui.button("TESTING");
                ui.button("TESTIN34G");
                ui.button("TESTIN34G");
                ui.button("TESTIN34G");
            });
            if ui.button("TESTING").clicked() {
                println!("button!!!!");
                println!("button!!!!");
                println!("button!!!!");
                println!("button!!!!");
            }
            ui.label("Hello world");
        });
    }
}
