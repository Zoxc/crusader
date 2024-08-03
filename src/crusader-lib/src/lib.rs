#![allow(
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::useless_format,
    clippy::type_complexity,
    clippy::collapsible_else_if,
    clippy::option_map_unit_fn
)]

pub const LIB_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn with_time(msg: &str) -> String {
    let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    format!("[{}] {}", time, msg)
}

pub mod file_format;
pub mod latency;
mod peer;
pub mod plot;
pub mod protocol;
pub mod remote;
pub mod serve;
pub mod test;
