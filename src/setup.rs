use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::cli::{SetupPlan, SetupTransaction};
use crate::cli_exit::CliExit as ExitCode;
use crate::config::Config;
use crate::metrics::xuva_data_root;
use crate::providers::cache::unix_seconds;
use crate::providers::commands::is_safe_provider_tool_name;
use crate::providers::discovery::first_windows_executable;
use crate::providers::model::{ProjectLocationKind, ProviderResolution};
use crate::providers::resolution::resolve_tool_provider;
use crate::state;

pub(crate) fn has_complete_go_provider(resolution: &ProviderResolution) -> bool {
    if resolution.project.kind != ProjectLocationKind::Wsl
        && resolution.availability.windows.executable.is_some()
    {
        return true;
    }
    resolution.candidates.iter().any(|candidate| {
        candidate.usable && candidate.is_wsl() && candidate.has_consistent_location()
    })
}

pub(crate) fn setup_go_plan_from_resolution(
    resolution: &ProviderResolution,
    winget_available: bool,
) -> SetupPlan {
    let verification_command = vec![
        "xuva".to_owned(),
        "doctor".to_owned(),
        "go".to_owned(),
        "--refresh".to_owned(),
    ];
    if has_complete_go_provider(resolution) {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "ready",
            reason: "a complete existing Go provider is already available; no setup is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    if resolution.project.kind == ProjectLocationKind::Windows
        && resolution.availability.windows.native_rtk.is_some()
        && winget_available
    {
        return SetupPlan {
            schema_version: 1,
            tool: "go".to_owned(),
            mode: "plan-only",
            status: "planned",
            reason: "Windows Go is absent while native RTK is already available".to_owned(),
            proposed_provider: Some("windows-winget"),
            proposed_command: Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ]),
            verification_command,
            apply: "unavailable_in_pd4",
        };
    }
    let reason = if resolution.project.kind == ProjectLocationKind::Wsl {
        "no complete provider is available for this WSL project; PD4 will not install a Windows toolchain across hosts".to_owned()
    } else if resolution.availability.windows.native_rtk.is_none() {
        "Windows Go setup is blocked because a verified native RTK provider is also required and is not available".to_owned()
    } else {
        "Windows Go setup is blocked because winget is unavailable; no alternate installer is selected automatically".to_owned()
    };
    SetupPlan {
        schema_version: 1,
        tool: "go".to_owned(),
        mode: "plan-only",
        status: "blocked",
        reason,
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_in_pd4",
    }
}

pub(crate) fn setup_generic_plan_from_resolution(resolution: &ProviderResolution) -> SetupPlan {
    let verification_command = vec![
        "xuva".to_owned(),
        "doctor".to_owned(),
        resolution.tool.clone(),
        "--refresh".to_owned(),
    ];
    if resolution.recommended.is_some() {
        return SetupPlan {
            schema_version: 1,
            tool: resolution.tool.clone(),
            mode: "diagnostic-only",
            status: "ready",
            reason: "a verified existing provider is available; no setup action is needed"
                .to_owned(),
            proposed_provider: None,
            proposed_command: None,
            verification_command,
            apply: "not_needed",
        };
    }
    SetupPlan {
        schema_version: 1,
        tool: resolution.tool.clone(),
        mode: "diagnostic-only",
        status: "blocked",
        reason: format!(
            "{}; XUVA will not guess an installer, package manager, or dependency chain for a generic tool",
            resolution.diagnosis
        ),
        proposed_provider: None,
        proposed_command: None,
        verification_command,
        apply: "unavailable_for_generic_tool",
    }
}

pub(crate) fn print_setup_plan(plan: &SetupPlan, json: bool) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(plan) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("xuva: unable to render setup plan: {error}");
                ExitCode::FAILURE
            }
        };
    }
    println!("tool={}", plan.tool);
    println!("mode={}", plan.mode);
    println!("status={}", plan.status);
    println!("reason={}", plan.reason);
    if let Some(provider) = plan.proposed_provider {
        println!("proposed_provider={provider}");
    }
    if let Some(command) = &plan.proposed_command {
        println!("proposed_command={}", command.join(" "));
    }
    println!(
        "verification_command={}",
        plan.verification_command.join(" ")
    );
    println!("apply={}", plan.apply);
    ExitCode::SUCCESS
}

pub(crate) fn setup_transaction_path() -> PathBuf {
    xuva_data_root().join("setup-transaction-v1.json")
}

pub(crate) fn load_setup_transaction() -> Option<SetupTransaction> {
    fs::read_to_string(setup_transaction_path())
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

pub(crate) fn write_setup_transaction(transaction: &SetupTransaction) -> Result<(), String> {
    state::write_json_atomic(&setup_transaction_path(), transaction, "setup transaction")
}

pub(crate) fn print_setup_transaction(
    transaction: Option<&SetupTransaction>,
    json: bool,
) -> ExitCode {
    if json {
        return match serde_json::to_string_pretty(&transaction) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("xuva: unable to render setup transaction: {error}");
                ExitCode::FAILURE
            }
        };
    }
    match transaction {
        Some(transaction) => {
            println!("tool={}", transaction.tool);
            println!("status={}", transaction.status);
            println!(
                "observed_unix_seconds={}",
                transaction.observed_unix_seconds
            );
            println!("detail={}", transaction.detail);
            if let Some(command) = &transaction.command {
                println!("command={}", command.join(" "));
            }
        }
        None => println!("No local setup transaction is recorded."),
    }
    ExitCode::SUCCESS
}

pub(crate) fn record_setup_transaction(
    status: &str,
    command: Option<Vec<String>>,
    detail: impl Into<String>,
) -> Result<SetupTransaction, String> {
    let transaction = SetupTransaction {
        schema_version: 1,
        tool: "go".to_owned(),
        status: status.to_owned(),
        observed_unix_seconds: unix_seconds(),
        command,
        detail: detail.into(),
    };
    write_setup_transaction(&transaction)?;
    Ok(transaction)
}

pub(crate) fn setup_recovery_outcome(has_complete_provider: bool) -> (&'static str, &'static str) {
    if has_complete_provider {
        (
            "recovered_verified",
            "fresh provider discovery found a complete Go provider; no installer was replayed",
        )
    } else {
        (
            "recovery_required",
            "fresh provider discovery is still incomplete; no installer was replayed and manual review is required",
        )
    }
}

pub(crate) fn recover_setup_transaction(config: &Config, json: bool) -> ExitCode {
    let Some(previous) = load_setup_transaction() else {
        return print_setup_transaction(None, json);
    };
    let resolution = resolve_tool_provider("go", config, true);
    let (status, detail) = setup_recovery_outcome(has_complete_go_provider(&resolution));
    let recovered = match record_setup_transaction(status, previous.command, detail) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("xuva: {error}");
            return ExitCode::FAILURE;
        }
    };
    print_setup_transaction(Some(&recovered), json)
}

pub(crate) fn apply_setup_plan(plan: &SetupPlan, config: &Config, json: bool) -> ExitCode {
    if plan.status == "ready" {
        return print_setup_plan(plan, json);
    }
    let Some(command) = plan.proposed_command.clone() else {
        eprintln!("xuva: setup is blocked; no installer is selected automatically");
        return ExitCode::FAILURE;
    };
    if let Err(error) = record_setup_transaction(
        "running",
        Some(command.clone()),
        "installer started after explicit --apply --confirm",
    ) {
        eprintln!("xuva: {error}");
        return ExitCode::FAILURE;
    }
    let mut installer = Command::new(&command[0]);
    installer.args(&command[1..]);
    let status = match installer.status() {
        Ok(status) => status,
        Err(error) => {
            let detail = format!("installer could not start: {error}");
            let _ = record_setup_transaction("failed", Some(command), &detail);
            eprintln!("xuva: {detail}");
            return ExitCode::FAILURE;
        }
    };
    if !status.success() {
        let detail = format!("installer exited with {status}");
        let _ = record_setup_transaction("failed", Some(command), &detail);
        eprintln!(
            "xuva: {detail}; run `xuva setup go --recover` to re-discover without replaying it"
        );
        return ExitCode::FAILURE;
    }
    let resolution = resolve_tool_provider("go", config, true);
    if has_complete_go_provider(&resolution) {
        let transaction = match record_setup_transaction(
            "verified",
            Some(command),
            "installer completed and fresh provider discovery found a complete Go provider",
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                eprintln!("xuva: {error}");
                return ExitCode::FAILURE;
            }
        };
        return print_setup_transaction(Some(&transaction), json);
    }
    let detail = "installer completed but fresh provider discovery is incomplete; reopen the shell if PATH changed, then run `xuva setup go --recover`";
    let _ = record_setup_transaction("verification_required", Some(command), detail);
    eprintln!("xuva: {detail}");
    ExitCode::FAILURE
}

pub(crate) fn setup_command(arguments: &[OsString], config: &Config) -> ExitCode {
    let Some(tool) = arguments.get(1).and_then(|argument| argument.to_str()) else {
        eprintln!(
            "xuva: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    };
    if !is_safe_provider_tool_name(tool) {
        eprintln!("xuva: tool names must contain only ASCII letters, digits, '.', '_', or '-'");
        return ExitCode::FAILURE;
    }
    let flags: Vec<&str> = match arguments
        .iter()
        .skip(2)
        .map(|argument| argument.to_str())
        .collect()
    {
        Some(flags) => flags,
        None => {
            eprintln!("xuva: setup options must be valid Unicode");
            return ExitCode::FAILURE;
        }
    };
    let valid = [
        "--json",
        "--refresh",
        "--status",
        "--recover",
        "--apply",
        "--confirm",
    ];
    if flags.iter().any(|flag| !valid.contains(flag)) {
        eprintln!(
            "xuva: usage: setup <tool> [--json] [--refresh]; setup go also supports [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    let json = flags.contains(&"--json");
    let refresh = flags.contains(&"--refresh");
    let status = flags.contains(&"--status");
    let recover = flags.contains(&"--recover");
    let apply = flags.contains(&"--apply");
    let confirm = flags.contains(&"--confirm");
    if tool != "go" {
        if status || recover || apply || confirm {
            eprintln!(
                "xuva: generic setup is diagnostic-only; `--apply`, `--confirm`, `--status`, and `--recover` are available only for the explicit Go transaction"
            );
            return ExitCode::FAILURE;
        }
        let resolution = resolve_tool_provider(tool, config, refresh);
        return print_setup_plan(&setup_generic_plan_from_resolution(&resolution), json);
    }
    if [status, recover, apply]
        .into_iter()
        .filter(|selected| *selected)
        .count()
        > 1
        || (confirm && !apply)
        || (status && refresh)
    {
        eprintln!(
            "xuva: usage: setup go [--json] [--refresh] [--status|--recover|--apply --confirm]"
        );
        return ExitCode::FAILURE;
    }
    if status {
        return print_setup_transaction(load_setup_transaction().as_ref(), json);
    }
    if recover {
        return recover_setup_transaction(config, json);
    }
    let resolution = resolve_tool_provider(tool, config, refresh || apply);
    let mut plan =
        setup_go_plan_from_resolution(&resolution, first_windows_executable("winget").is_some());
    if plan.status == "planned" {
        plan.apply = "requires_apply_and_confirm";
    }
    if !apply {
        return print_setup_plan(&plan, json);
    }
    if !confirm {
        eprintln!(
            "xuva: review the plan above; re-run with `xuva setup go --apply --confirm` to start the installer"
        );
        let _ = print_setup_plan(&plan, json);
        return ExitCode::from(2);
    }
    apply_setup_plan(&plan, config, json)
}

#[cfg(test)]
mod tests {
    use crate::providers::cache::PROVIDER_CACHE_SCHEMA_VERSION;
    use crate::providers::model::{
        InspectionLevel, ProbeStatus, ProjectLocation, ProjectLocationKind, ProviderCacheEntry,
        ProviderResolution, WindowsToolProbe,
    };

    use super::*;

    #[test]
    fn setup_plan_proposes_only_a_reviewable_windows_go_command() {
        let resolution = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: Some(r"C:\tools\rtk.exe".to_owned()),
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: no provider is available".to_owned(),
            install: "disabled_in_pd1",
        };
        let plan = setup_go_plan_from_resolution(&resolution, true);
        assert_eq!(plan.status, "planned");
        assert_eq!(plan.proposed_provider, Some("windows-winget"));
        assert_eq!(plan.apply, "unavailable_in_pd4");
        assert_eq!(
            plan.proposed_command,
            Some(vec![
                "winget".to_owned(),
                "install".to_owned(),
                "--id".to_owned(),
                "GoLang.Go".to_owned(),
                "--exact".to_owned(),
                "--source".to_owned(),
                "winget".to_owned(),
                "--accept-package-agreements".to_owned(),
                "--accept-source-agreements".to_owned(),
            ])
        );
    }

    #[test]
    fn setup_plan_never_selects_an_installer_when_a_provider_is_ready_or_blocked() {
        let ready = ProviderResolution {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            tool: "go".to_owned(),
            cache: "miss",
            project: ProjectLocation {
                kind: ProjectLocationKind::Windows,
                path: r"E:\work".to_owned(),
                distro: None,
                windows_path: None,
            },
            availability: ProviderCacheEntry {
                tool: "go".to_owned(),
                observed_unix_seconds: 1,
                inspection_level: InspectionLevel::Identity,
                context_signature: "fixture".to_owned(),
                windows: WindowsToolProbe {
                    executable: Some(r"C:\Go\bin\go.exe".to_owned()),
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                wsl_probe_complete: true,
                wsl: Vec::new(),
            },
            candidates: Vec::new(),
            recommended: None,
            diagnosis: "fixture: Windows Go is available".to_owned(),
            install: "disabled_in_pd1",
        };
        let ready_plan = setup_go_plan_from_resolution(&ready, false);
        assert_eq!(ready_plan.status, "ready");
        assert_eq!(ready_plan.proposed_command, None);
        assert_eq!(ready_plan.apply, "not_needed");

        let blocked = ProviderResolution {
            availability: ProviderCacheEntry {
                windows: WindowsToolProbe {
                    executable: None,
                    native_rtk: None,
                    executable_version: None,
                    version_probe_status: ProbeStatus::NotRequested,
                    executable_capabilities: Vec::new(),
                    executable_identity: None,
                    native_rtk_identity: None,
                },
                ..ready.availability.clone()
            },
            ..ready
        };
        let blocked_plan = setup_go_plan_from_resolution(&blocked, true);
        assert_eq!(blocked_plan.status, "blocked");
        assert_eq!(blocked_plan.proposed_command, None);
        assert_eq!(blocked_plan.apply, "unavailable_in_pd4");
    }

    #[test]
    fn setup_recovery_never_replays_an_installer() {
        let (verified_status, verified_detail) = setup_recovery_outcome(true);
        assert_eq!(verified_status, "recovered_verified");
        assert!(verified_detail.contains("no installer was replayed"));

        let (required_status, required_detail) = setup_recovery_outcome(false);
        assert_eq!(required_status, "recovery_required");
        assert!(required_detail.contains("no installer was replayed"));
    }
}
