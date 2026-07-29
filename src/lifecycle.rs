//! Installed-launcher lifecycle commands.
//!
//! Windows does not allow a running executable to replace or remove itself.
//! Rollback and uninstall therefore start a minimal helper that waits for this
//! process to exit and then invokes the installed, reviewed companion script.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::PRODUCT_COMMAND;

const HELPER_SCRIPT: &str = r#"
param(
    [Parameter(Mandatory)] [int]$ParentId,
    [Parameter(Mandatory)] [string]$ScriptPath,
    [Parameter(Mandatory)] [string]$Destination,
    [Parameter(Mandatory)] [string]$Action,
    [Parameter(Mandatory)] [string]$LogPath
)
$ErrorActionPreference = 'Stop'
try {
    Wait-Process -Id $ParentId -ErrorAction SilentlyContinue
    if ($Action -eq 'rollback') {
        & $ScriptPath -Destination $Destination -Rollback
    } elseif ($Action -eq 'uninstall') {
        & $ScriptPath -Destination $Destination
    } elseif ($Action -eq 'uninstall-path') {
        & $ScriptPath -Destination $Destination -RemoveFromPath
    } else {
        throw 'Unsupported XUVA lifecycle action.'
    }
    Set-Content -LiteralPath $LogPath -Value "status=success`naction=$Action" -Encoding ascii
} catch {
    Set-Content -LiteralPath $LogPath -Value "status=failed`naction=$Action`nerror=$($_.Exception.Message)" -Encoding utf8
    exit 1
} finally {
    Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#;

fn installed_paths() -> Result<(PathBuf, PathBuf), String> {
    let executable =
        env::current_exe().map_err(|error| format!("unable to locate the XUVA binary: {error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| "the XUVA binary has no installation directory".to_owned())?
        .to_owned();
    Ok((executable, directory))
}

fn path_contains(directory: &Path) -> bool {
    env::split_paths(&env::var_os("PATH").unwrap_or_default()).any(|entry| {
        entry
            .canonicalize()
            .ok()
            .zip(directory.canonicalize().ok())
            .is_some_and(|(entry, directory)| entry == directory)
    })
}

fn print_status() -> ExitCode {
    let (executable, directory) = match installed_paths() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{PRODUCT_COMMAND}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = serde_json::json!({
        "executable": executable,
        "directory": directory,
        "backup_available": PathBuf::from(format!("{}.previous.exe", executable.display())).is_file(),
        "installer_available": directory.join("install.ps1").is_file(),
        "uninstaller_available": directory.join("uninstall.ps1").is_file(),
        "on_process_path": path_contains(&directory),
    });
    match serde_json::to_string_pretty(&status) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{PRODUCT_COMMAND}: unable to render installation status: {error}");
            ExitCode::FAILURE
        }
    }
}

fn powershell_path() -> OsString {
    env::var_os("SYSTEMROOT")
        .map(PathBuf::from)
        .map(|root| {
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
                .into_os_string()
        })
        .unwrap_or_else(|| OsString::from("powershell.exe"))
}

fn schedule(action: &str, script_name: &str) -> ExitCode {
    let (executable, directory) = match installed_paths() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{PRODUCT_COMMAND}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let script = directory.join(script_name);
    if !script.is_file() {
        eprintln!(
            "{PRODUCT_COMMAND}: installed companion `{}` is missing; reinstall from the verified release archive",
            script.display()
        );
        return ExitCode::FAILURE;
    }
    if action == "rollback"
        && !PathBuf::from(format!("{}.previous.exe", executable.display())).is_file()
    {
        eprintln!(
            "{PRODUCT_COMMAND}: no previous launcher backup is available beside {}",
            executable.display()
        );
        return ExitCode::FAILURE;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let helper = env::temp_dir().join(format!("xuva-lifecycle-{}-{nonce}.ps1", std::process::id()));
    let log = env::temp_dir().join(format!("xuva-lifecycle-{}-{nonce}.log", std::process::id()));
    if let Err(error) = fs::write(&helper, HELPER_SCRIPT) {
        eprintln!("{PRODUCT_COMMAND}: unable to prepare the lifecycle helper: {error}");
        return ExitCode::FAILURE;
    }
    let mut command = Command::new(powershell_path());
    command
        .args([
            OsString::from("-NoLogo"),
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-File"),
            helper.clone().into_os_string(),
            OsString::from("-ParentId"),
            OsString::from(std::process::id().to_string()),
            OsString::from("-ScriptPath"),
            script.into_os_string(),
            OsString::from("-Destination"),
            directory.into_os_string(),
            OsString::from("-Action"),
            OsString::from(action),
            OsString::from("-LogPath"),
            log.clone().into_os_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(_) => {
            println!(
                "{action} scheduled; it will run after this XUVA process exits. Result: {}",
                log.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = fs::remove_file(helper);
            eprintln!("{PRODUCT_COMMAND}: unable to schedule {action}: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn command(arguments: &[OsString]) -> Option<ExitCode> {
    match arguments {
        [command, option] if command == "install" && option == "--status" => Some(print_status()),
        [command] if command == "rollback" => Some(schedule("rollback", "install.ps1")),
        [command] if command == "uninstall" => Some(schedule("uninstall", "uninstall.ps1")),
        [command, option] if command == "uninstall" && option == "--remove-from-path" => {
            Some(schedule("uninstall-path", "uninstall.ps1"))
        }
        [command] if command == "install" => {
            eprintln!(
                "{PRODUCT_COMMAND}: use the verified archive's install.ps1; inspect with `{PRODUCT_COMMAND} install --status`"
            );
            Some(ExitCode::FAILURE)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_bounded_lifecycle_commands() {
        assert!(command(&[OsString::from("install"), OsString::from("--status")]).is_some());
        assert!(command(&[OsString::from("install"), OsString::from("--force")]).is_none());
        assert!(command(&[OsString::from("rollback"), OsString::from("extra")]).is_none());
    }
}
