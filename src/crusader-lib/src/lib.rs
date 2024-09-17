#![allow(
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::useless_format,
    clippy::type_complexity,
    clippy::collapsible_else_if,
    clippy::option_map_unit_fn
)]

const VERSION: &str = "0.3.1-dev";

pub fn version() -> String {
    if !VERSION.ends_with("-dev") {
        VERSION.to_owned()
    } else {
        let commit = option_env!("GIT_COMMIT")
            .map(|commit| format!("commit {}", commit))
            .unwrap_or("unknown commit".to_owned());
        format!("{} ({})", VERSION, commit)
    }
}

pub fn with_time(msg: &str) -> String {
    let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    format!("[{}] {}", time, msg)
}

mod common;
mod discovery;
#[cfg(feature = "client")]
pub use common::Config;
#[cfg(feature = "client")]
pub mod file_format;
#[cfg(feature = "client")]
pub mod latency;
mod peer;
#[cfg(feature = "client")]
pub mod plot;
pub mod protocol;
#[cfg(feature = "client")]
pub mod remote;
pub mod serve;
#[cfg(feature = "client")]
pub mod test;
