#![allow(
    clippy::new_without_default,
    clippy::too_many_arguments,
    clippy::useless_format,
    clippy::type_complexity,
    clippy::collapsible_else_if,
    clippy::option_map_unit_fn
)]

pub mod file_format;
pub mod latency;
mod peer;
pub mod plot;
pub mod protocol;
pub mod serve;
pub mod test;
