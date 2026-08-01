use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::diagnostics::trace;
use crate::process;
use crate::providers::discovery::decode_wsl_output;
use crate::wsl::authorization::dedicated_wsl1_installation_id_for;

pub(crate) fn revalidate_dedicated_wsl1_installation(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match dedicated_wsl1_installation_id_for(&config.distro) {
            Some(actual) if actual == expected_installation_id => return Ok(()),
            Some(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the WSL1 dedicated-runtime identity changed after child launch",
                ));
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unable to revalidate the WSL1 dedicated-runtime identity before termination",
                ));
            }
        }
    }
}

pub(crate) fn running_wsl_distributions() -> std::io::Result<Vec<String>> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--running", "--quiet"]);
    let output = process::run_probe(&mut command)?;
    if !output.status.success() || output.stdout_truncated {
        return Err(std::io::Error::other(
            "unable to inspect running WSL distributions",
        ));
    }
    Ok(decode_wsl_output(&output.stdout)
        .replace('\0', "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(crate) fn wait_for_wsl_distro_to_stop_within(
    distro: &str,
    timeout: Duration,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let running = running_wsl_distributions()?;
        if !running.iter().any(|candidate| candidate == distro) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("WSL distro {distro} remained running after termination"),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn wait_for_wsl_distro_to_stop(distro: &str) -> std::io::Result<()> {
    wait_for_wsl_distro_to_stop_within(distro, Duration::from_secs(5))
}

pub(crate) fn terminate_dedicated_wsl1_distro(
    config: &Config,
    expected_installation_id: &str,
) -> std::io::Result<()> {
    revalidate_dedicated_wsl1_installation(config, expected_installation_id)?;
    trace(format!(
        "terminating dedicated WSL1 distro {} installation {} after cancellation",
        config.distro, expected_installation_id
    ));
    let mut command = Command::new("wsl.exe");
    command.args(["--terminate", &config.distro]);
    match process::run_probe(&mut command) {
        Ok(output) if output.status.success() => wait_for_wsl_distro_to_stop(&config.distro),
        Ok(output) => {
            trace(format!(
                "WSL1 terminate returned {}: {}{}",
                output.status,
                decode_wsl_output(&output.stdout).trim(),
                decode_wsl_output(&output.stderr).trim()
            ));
            Err(std::io::Error::other("WSL1 terminate command failed"))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn stop_cancelled_wsl1_child(
    child: &mut Child,
    config: &Config,
    expected_installation_id: Option<&str>,
) -> std::io::Result<ExitStatus> {
    // Stop the Windows proxy first so it cannot outlive XUVA. The Linux-side
    // command is still blocked on the identity-bound permit at this point.
    let _ = child.kill();
    let termination = expected_installation_id
        .map(|installation_id| terminate_dedicated_wsl1_distro(config, installation_id));
    let proxy_deadline = Instant::now() + Duration::from_secs(3);
    let proxy_status = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }
        if Instant::now() >= proxy_deadline {
            break Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the Windows WSL1 proxy remained alive after cancellation",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };

    match termination {
        Some(Err(error)) => {
            // An identity change means XUVA must not terminate a potentially
            // unrelated distro. The unpermitted child self-expires instead,
            // and XUVA proves that the runtime stopped before returning the
            // failure.
            let stopped =
                wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12));
            return Err(match stopped {
                Ok(()) => error,
                Err(stop_error) => std::io::Error::other(format!(
                    "{error}; the untrusted WSL1 runtime also failed to stop: {stop_error}"
                )),
            });
        }
        None => {
            wait_for_wsl_distro_to_stop_within(&config.distro, Duration::from_secs(12)).map_err(
                |stop_error| {
                    std::io::Error::other(format!(
                        "the unpermitted WSL1 runtime failed to stop after proxy cancellation: {stop_error}"
                    ))
                },
            )?;
        }
        Some(Ok(())) => {}
    }
    proxy_status
}
