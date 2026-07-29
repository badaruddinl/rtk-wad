//! Device-specific process adapters.
//!
//! Discovery chooses a device; these modules own how that device starts a
//! process. Shared command semantics remain structured and never use a user
//! supplied shell string.

use std::ffi::OsString;
use std::process::Command;

pub(crate) mod rtk;
pub(crate) mod windows;
pub(crate) mod wsl1;
pub(crate) mod wsl2;

fn wsl_command(arguments: Vec<OsString>) -> Command {
    let mut command = Command::new("wsl.exe");
    command.args(arguments);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    command
}
