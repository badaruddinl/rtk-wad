//! WSL2 device transport.

use std::ffi::OsString;
use std::process::Command;

pub(crate) fn process(arguments: Vec<OsString>) -> Command {
    super::wsl_command(arguments)
}
