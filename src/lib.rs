pub const PRODUCT_NAME: &str = "XUVA";
pub const PRODUCT_COMMAND: &str = "xuva";

pub mod adapters;
pub mod agent;
pub mod app;
pub mod bridge;
pub mod cli;
pub mod cli_exit;
pub mod config;
pub mod diagnostics;
pub mod dispatcher;
pub mod execution;
pub mod lifecycle;
pub mod metrics;
pub mod paths;
pub mod planning;
pub mod process;
pub mod providers;
pub mod routing;
pub mod self_update;
pub mod setup;
pub mod state;
pub mod wsl;

#[cfg(test)]
mod test_support;
