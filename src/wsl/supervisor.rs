use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::diagnostics::trace;
use crate::wsl::authorization::{
    LaunchPermitGuard, verify_pre_authorization_proxy_status, verify_proxy_completion_status,
};
use crate::wsl::cancellation::{
    LinuxProcessGroupState, console, linux_process_group_state, send_linux_signal,
};
use crate::wsl::lifecycle::stop_cancelled_wsl1_child;
use crate::wsl::test_hooks::{
    test_defer_wsl2_proxy_reap_until_cleanup, test_kill_wsl1_proxy_after_permit,
    test_kill_wsl2_proxy_during_cancellation, test_ready_file_exists,
};
use crate::wsl::valid_installation_id;

pub(crate) fn wait_for_wsl1_child(
    mut child: Child,
    config: &Config,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    let mut authorized = false;
    let mut accepted_installation_id = None;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    loop {
        let cancellation_requested = console::requested();
        if cancellation_requested {
            match (
                accepted_installation_id.as_deref(),
                launch_guard.attested_value(),
            ) {
                (Some(installation_id), _) => {
                    return stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                }
                (None, Ok(Some(installation_id))) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id);
                    return stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                }
                (None, Ok(Some(_))) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the cancelled WSL1 child attested an invalid installation ID",
                    ));
                }
                (None, Err(error)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
                (None, Ok(None)) if started.elapsed() < Duration::from_secs(10) => {
                    // The target remains blocked because no permit is ever
                    // published. Keep the proxy alive long enough for the
                    // root-owned dedicated marker to be attested, then use
                    // that exact identity for safe distro termination.
                }
                (None, Ok(None)) => {
                    let cleanup = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(match cleanup {
                        Ok(_) => std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "cancelled WSL1 child never attested a dedicated-runtime identity",
                        ),
                        Err(cleanup_error) => cleanup_error,
                    });
                }
            }
        }
        if !authorized && !cancellation_requested {
            match launch_guard.attested_value() {
                Ok(Some(installation_id)) if valid_installation_id(&installation_id) => {
                    accepted_installation_id = Some(installation_id.clone());
                    if let Err(error) = launch_guard.authorize_value(&installation_id) {
                        let cleanup = stop_cancelled_wsl1_child(
                            &mut child,
                            config,
                            accepted_installation_id.as_deref(),
                        );
                        return Err(match cleanup {
                            Ok(_) => error,
                            Err(cleanup_error) => std::io::Error::other(format!(
                                "{error}; WSL1 authorization cleanup failed: {cleanup_error}"
                            )),
                        });
                    }
                    authorized = true;
                    trace("authorized an identity-matched WSL1 child");
                }
                Ok(Some(_)) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "the WSL1 child attested an invalid dedicated-runtime installation ID",
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = stop_cancelled_wsl1_child(&mut child, config, None);
                    return Err(error);
                }
            }
        }
        if authorized
            && !test_proxy_killed
            && test_kill_wsl1_proxy_after_permit()
            && test_ready_file_exists()
        {
            // The test-only ready boundary is published immediately before
            // target launch. Give the target one scheduler turn so the
            // contract exercises a proxy failure after execution has begun.
            thread::sleep(Duration::from_millis(100));
            trace("test hook terminated the WSL1 Windows proxy after launch permit");
            let _ = child.kill();
            test_proxy_killed = true;
        }
        if proxy_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    trace(format!("WSL1 Windows proxy exited with {status}"));
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
                Ok(None) => {}
                Err(error) => {
                    let cleanup = stop_cancelled_wsl1_child(
                        &mut child,
                        config,
                        accepted_installation_id.as_deref(),
                    );
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 child status cleanup failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if let Some(status) = proxy_status {
            if !authorized {
                return verify_pre_authorization_proxy_status(status);
            }
            let installation_id = accepted_installation_id.as_deref().ok_or_else(|| {
                std::io::Error::other(
                    "an authorized WSL1 child has no accepted dedicated-runtime identity",
                )
            })?;
            match launch_guard.completion_status_for(installation_id) {
                Ok(Some(attested_status)) => {
                    match verify_proxy_completion_status(status, attested_status) {
                        Ok(status) => return Ok(status),
                        Err(error) => {
                            let cleanup = stop_cancelled_wsl1_child(
                                &mut child,
                                config,
                                Some(installation_id),
                            );
                            return Err(match cleanup {
                                Ok(_) => error,
                                Err(cleanup_error) => std::io::Error::other(format!(
                                    "{error}; WSL1 mismatch recovery failed: {cleanup_error}"
                                )),
                            });
                        }
                    }
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    let error = std::io::Error::other(
                        "WSL1 proxy exited after launch permit without a completion attestation",
                    );
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 incomplete-launch recovery failed: {cleanup_error}"
                        )),
                    });
                }
                Err(error) => {
                    let cleanup =
                        stop_cancelled_wsl1_child(&mut child, config, Some(installation_id));
                    return Err(match cleanup {
                        Ok(_) => error,
                        Err(cleanup_error) => std::io::Error::other(format!(
                            "{error}; WSL1 invalid-completion recovery failed: {cleanup_error}"
                        )),
                    });
                }
            }
        }
        if !authorized && started.elapsed() >= Duration::from_secs(10) {
            let _ =
                stop_cancelled_wsl1_child(&mut child, config, accepted_installation_id.as_deref());
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "WSL1 child did not attest its dedicated-runtime identity within 10 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn wait_for_wsl_child(
    mut child: Child,
    config: &Config,
    token: &str,
    launch_guard: &LaunchPermitGuard,
) -> std::io::Result<ExitStatus> {
    let launched_at = Instant::now();
    let mut authorized = false;
    let mut pending_error: Option<std::io::Error> = None;
    let mut cancellation_started: Option<Instant> = None;
    let mut interrupt_sent = false;
    let mut terminate_sent = false;
    let mut kill_sent = false;
    let mut proxy_status = None;
    let mut proxy_exited_at = None;
    let mut test_proxy_killed = false;
    let mut test_proxy_reap_deferred = false;
    let test_defer_proxy_reap = test_defer_wsl2_proxy_reap_until_cleanup();
    if test_defer_proxy_reap {
        trace("test hook armed deferred WSL2 proxy reap");
    }
    loop {
        if console::requested() {
            cancellation_started.get_or_insert_with(Instant::now);
        }
        let defer_proxy_reap = test_proxy_killed && test_defer_proxy_reap;
        if defer_proxy_reap && !test_proxy_reap_deferred {
            trace("test hook deferred WSL2 proxy reap until Linux cleanup");
            test_proxy_reap_deferred = true;
        }
        if proxy_status.is_none()
            && !defer_proxy_reap
            && let Some(status) = child.try_wait()?
        {
            trace(format!("WSL2 Windows proxy exited with {status}"));
            proxy_status = Some(status);
            proxy_exited_at = Some(Instant::now());
        }
        if !authorized && cancellation_started.is_none() && proxy_status.is_none() {
            match launch_guard.is_attested() {
                Ok(true) => match launch_guard.authorize() {
                    Ok(()) => {
                        authorized = true;
                        trace("authorized a cancellation-ready WSL2 child");
                    }
                    Err(error) => {
                        pending_error = Some(error);
                        cancellation_started = Some(Instant::now());
                    }
                },
                Ok(false) if launched_at.elapsed() < Duration::from_secs(10) => {}
                Ok(false) => {
                    pending_error = Some(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "WSL2 child did not attest its private cancellation token within 10 seconds",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(status) = proxy_status.as_ref()
            && cancellation_started.is_none()
            && pending_error.is_none()
        {
            match launch_guard.completion_status() {
                Ok(Some(attested_status)) => {
                    return verify_proxy_completion_status(*status, attested_status);
                }
                Ok(None)
                    if proxy_exited_at
                        .is_some_and(|exited| exited.elapsed() < Duration::from_millis(500)) => {}
                Ok(None) => {
                    pending_error = Some(std::io::Error::other(
                        "WSL2 proxy exited before the Linux launcher attested complete process-group cleanup",
                    ));
                    cancellation_started = Some(Instant::now());
                }
                Err(error) => {
                    pending_error = Some(error);
                    cancellation_started = Some(Instant::now());
                }
            }
        }
        if let Some(started) = cancellation_started {
            let elapsed = started.elapsed();
            if test_kill_wsl2_proxy_during_cancellation()
                && !test_proxy_killed
                && proxy_status.is_none()
            {
                trace("test hook terminated the WSL2 Windows proxy during cancellation");
                let _ = child.kill();
                test_proxy_killed = true;
            }
            if !interrupt_sent && send_linux_signal(config, token, "INT").unwrap_or(false) {
                trace("sent SIGINT to the isolated Linux process group");
                interrupt_sent = true;
            }
            if elapsed >= Duration::from_millis(1_500)
                && !terminate_sent
                && send_linux_signal(config, token, "TERM").unwrap_or(false)
            {
                trace("escalated cancellation to SIGTERM inside Linux");
                terminate_sent = true;
            }
            if elapsed >= Duration::from_secs(3)
                && !kill_sent
                && send_linux_signal(config, token, "KILL").unwrap_or(false)
            {
                trace("escalated cancellation to SIGKILL inside Linux");
                kill_sent = true;
            }
            let completion = match launch_guard.completion_status() {
                Ok(status) => status,
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let group_state = match linux_process_group_state(config, token) {
                Ok(state) => Some(state),
                Err(error) => {
                    if pending_error.is_none() {
                        pending_error = Some(error);
                    }
                    None
                }
            };
            let cleanup_proven = matches!(group_state, Some(LinuxProcessGroupState::Gone))
                || matches!(
                    (group_state, completion),
                    (Some(LinuxProcessGroupState::TokenUnavailable), Some(_))
                );
            if cleanup_proven {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                if let Some(error) = pending_error {
                    return Err(error);
                }
                return Ok(status);
            }
            if elapsed >= Duration::from_secs(4) && proxy_status.is_none() {
                let _ = child.kill();
                if let Some(status) = child.try_wait()? {
                    proxy_status = Some(status);
                    proxy_exited_at = Some(Instant::now());
                }
            }
            if elapsed >= Duration::from_secs(15) {
                let status = if let Some(status) = proxy_status {
                    status
                } else {
                    let _ = child.kill();
                    child.wait()?
                };
                trace(format!(
                    "reaped WSL2 Windows proxy with {status} after failed cleanup proof"
                ));
                let cleanup_error = std::io::Error::other(match group_state {
                    Some(LinuxProcessGroupState::Alive) => {
                        "Linux process group survived SIGINT, SIGTERM, and SIGKILL escalation"
                    }
                    Some(LinuxProcessGroupState::TokenUnavailable) => {
                        "WSL2 cancellation token disappeared without a completion attestation"
                    }
                    Some(LinuxProcessGroupState::Gone) => {
                        "WSL2 cleanup completed without a reapable Windows proxy"
                    }
                    None => "unable to prove WSL2 process-group cleanup after proxy exit",
                });
                return Err(match pending_error {
                    Some(error) => std::io::Error::other(format!(
                        "{error}; cancellation finalization failed: {cleanup_error}"
                    )),
                    None => cleanup_error,
                });
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}
