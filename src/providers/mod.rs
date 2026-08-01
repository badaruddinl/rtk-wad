//! Provider discovery and verification contracts.

pub(crate) mod cache;
pub(crate) mod commands;
pub(crate) mod discovery;
#[cfg(test)]
mod discovery_tests;
pub(crate) mod dispatch;
#[cfg(test)]
mod dispatch_tests;
pub(crate) mod mapping;
pub(crate) mod model;
pub(crate) mod probe;
pub(crate) mod resolution;
#[cfg(test)]
mod resolution_tests;
