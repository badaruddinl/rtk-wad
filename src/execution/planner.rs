use std::ffi::OsString;
use std::path::PathBuf;

use crate::adapters::rtk::{CommandSurface, command_surface};
use crate::config::{
    Config, DEFAULT_DISTRO, DEFAULT_WSL1_DISTRO, OutputAdapterPreference, Route, WslBackend,
};
use crate::execution::environment::forwarded_environment;
use crate::planning::{provider_environment_policy, windows_cwd_for_invocation};
use crate::providers::discovery::windows_binary_identity;
use crate::providers::mapping::{translate_arguments_to_windows, translate_arguments_to_wsl};
use crate::providers::model::{AdapterKind, ProviderCandidate, ProviderHost};
use crate::providers::resolution::requires_raw_posix_provider;
use crate::{adapters, dispatcher};

pub(crate) fn configured_wsl_backend(config: &Config, route: Route) -> Config {
    let mut selected = config.clone();
    match route {
        Route::Wsl1 => {
            selected.backend = WslBackend::Wsl1;
            if config.backend != WslBackend::Wsl1 && selected.distro == DEFAULT_DISTRO {
                selected.distro = DEFAULT_WSL1_DISTRO.to_owned();
            }
        }
        Route::Wsl2 => {
            selected.backend = WslBackend::Wsl2;
            if config.backend != WslBackend::Wsl2 && selected.distro == DEFAULT_WSL1_DISTRO {
                selected.distro = DEFAULT_DISTRO.to_owned();
            }
        }
        Route::Auto | Route::Raw | Route::NativeRtk => {}
    }
    selected
}

pub(crate) fn static_windows_execution_plan(
    arguments: &[OsString],
    config: &Config,
    route: Route,
) -> Result<dispatcher::ExecutionPlan, String> {
    let tool = arguments
        .first()
        .ok_or_else(|| "a Windows execution plan needs a command".to_owned())?;
    let tool_name = tool
        .to_str()
        .ok_or_else(|| "cross-host command names must be valid Unicode".to_owned())?;
    let cwd = windows_cwd_for_invocation(config)?;
    let raw_executable = adapters::windows::raw_executable(tool);
    let (candidate_executable, adapter) = match route {
        Route::Raw => (raw_executable, dispatcher::OutputAdapter::Raw),
        Route::NativeRtk => {
            let executable = OsString::from(&config.native_rtk_path);
            (
                executable.clone(),
                dispatcher::OutputAdapter::Rtk { executable },
            )
        }
        Route::Auto | Route::Wsl1 | Route::Wsl2 => {
            return Err("only Windows raw/native routes can use a static Windows plan".to_owned());
        }
    };
    let expected_identity = windows_binary_identity(&candidate_executable.to_string_lossy());
    Ok(dispatcher::ExecutionPlan {
        request: dispatcher::CommandSpec {
            executable: tool.clone(),
            arguments: translate_arguments_to_windows(tool_name, &arguments[1..], config),
            cwd: Some(cwd.clone()),
            environment: forwarded_environment(config),
            environment_policy: dispatcher::EnvironmentPolicy::Isolated,
            interactive: false,
        },
        candidate: dispatcher::RouteCandidate::Windows {
            executable: candidate_executable,
            cwd: Some(cwd),
        },
        adapter,
        expected_identity,
        explanation: vec![dispatcher::DecisionReason(
            "WSL-origin Windows execution uses an isolated structured plan".to_owned(),
        )],
    })
}

pub(crate) fn provider_adapter(
    candidate: &ProviderCandidate,
    preference: OutputAdapterPreference,
) -> Result<dispatcher::OutputAdapter, std::io::Error> {
    match (preference, candidate.rtk.as_deref()) {
        (OutputAdapterPreference::Raw, _) if candidate.supports_adapter(AdapterKind::Raw) => {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Raw, _) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "raw output was requested but this provider has no raw tool executable",
        )),
        (OutputAdapterPreference::Auto, None) if candidate.supports_adapter(AdapterKind::Raw) => {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Auto, None) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "this provider has neither a raw executable nor a usable output adapter",
        )),
        (OutputAdapterPreference::Auto | OutputAdapterPreference::Rtk, Some(executable))
            if candidate.supports_adapter(AdapterKind::Rtk) =>
        {
            Ok(dispatcher::OutputAdapter::Rtk {
                executable: OsString::from(executable),
            })
        }
        (OutputAdapterPreference::Rtk, _) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "RTK output adapter was requested but this provider has no RTK executable",
        )),
        (OutputAdapterPreference::Auto, Some(_))
            if candidate.supports_adapter(AdapterKind::Raw) =>
        {
            Ok(dispatcher::OutputAdapter::Raw)
        }
        (OutputAdapterPreference::Auto, Some(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the configured adapter is not usable and no raw tool executable exists",
        )),
    }
}

pub(crate) fn provider_execution_config(
    config: &Config,
    route: &dispatcher::RouteCandidate,
    adapter: &dispatcher::OutputAdapter,
) -> Result<Config, std::io::Error> {
    let (wsl_route, distro, cwd, raw_executable) = match route {
        dispatcher::RouteCandidate::Wsl1 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl1, distro, cwd, executable),
        dispatcher::RouteCandidate::Wsl2 {
            distro,
            cwd,
            executable,
        } => (Route::Wsl2, distro, cwd, executable),
        dispatcher::RouteCandidate::Windows { .. } => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a Windows execution plan has no WSL transport configuration",
            ));
        }
    };
    let mut selected = configured_wsl_backend(config, wsl_route);
    selected.distro = distro.clone();
    selected.cwd = Some(cwd.to_string_lossy().into_owned());
    selected.rtk_path = Some(match adapter {
        dispatcher::OutputAdapter::Raw => raw_executable.to_string_lossy().into_owned(),
        dispatcher::OutputAdapter::Rtk { executable } => executable.to_string_lossy().into_owned(),
    });
    Ok(selected)
}

pub(crate) fn execution_plan_for_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
) -> Result<dispatcher::ExecutionPlan, std::io::Error> {
    let cwd = candidate.project_path.as_deref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider candidate has no verified project directory",
        )
    })?;
    if !candidate.has_consistent_location() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "provider host, distribution, and WSL version are inconsistent",
        ));
    }
    let raw_required = (tool == "git" && candidate.is_windows())
        || requires_raw_posix_provider(tool)
        || (config.output_adapter == OutputAdapterPreference::Auto
            && matches!(
                command_surface(tool),
                CommandSurface::RawNative | CommandSurface::Unknown
            ));
    let adapter = if raw_required {
        if !candidate.supports_adapter(AdapterKind::Raw) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "this command requires raw semantics but the provider has no raw tool executable",
            ));
        }
        // Git for Windows owns NTFS worktrees, object creation, credentials,
        // and Windows network configuration. Keep it raw so an output adapter
        // cannot turn a successful native operation into a failed WSL one.
        dispatcher::OutputAdapter::Raw
    } else {
        provider_adapter(candidate, config.output_adapter)?
    };
    let translated_arguments = match candidate.host {
        ProviderHost::Windows => translate_arguments_to_windows(tool, arguments, config),
        ProviderHost::Wsl1 | ProviderHost::Wsl2 => translate_arguments_to_wsl(
            tool,
            arguments,
            config,
            candidate
                .distro
                .as_deref()
                .expect("consistent WSL candidates have a distro"),
        ),
    };
    let request = dispatcher::CommandSpec {
        executable: OsString::from(tool),
        arguments: translated_arguments,
        cwd: Some(PathBuf::from(cwd)),
        environment: forwarded_environment(config),
        environment_policy: provider_environment_policy(config, candidate),
        interactive: false,
    };
    let route = match candidate.host {
        ProviderHost::Windows => dispatcher::RouteCandidate::Windows {
            executable: OsString::from(&candidate.executable),
            cwd: Some(PathBuf::from(cwd)),
        },
        ProviderHost::Wsl1 => dispatcher::RouteCandidate::Wsl1 {
            distro: candidate
                .distro
                .clone()
                .expect("consistent WSL1 candidates have a distro"),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
        ProviderHost::Wsl2 => dispatcher::RouteCandidate::Wsl2 {
            distro: candidate
                .distro
                .clone()
                .expect("consistent WSL2 candidates have a distro"),
            executable: OsString::from(&candidate.executable),
            cwd: PathBuf::from(cwd),
        },
    };
    let expected_identity = match &adapter {
        dispatcher::OutputAdapter::Raw => candidate.executable_identity.clone(),
        dispatcher::OutputAdapter::Rtk { .. } => candidate.rtk_identity.clone(),
    }
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "provider executable has no captured binary identity",
        )
    })?;
    let identity_matches_plan = match &adapter {
        dispatcher::OutputAdapter::Raw => expected_identity.path == candidate.executable,
        dispatcher::OutputAdapter::Rtk { executable } => {
            executable == &OsString::from(&expected_identity.path)
        }
    };
    if !identity_matches_plan {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "provider executable path does not match its captured binary identity",
        ));
    }
    Ok(dispatcher::ExecutionPlan {
        request,
        candidate: route,
        adapter,
        expected_identity: Some(expected_identity),
        explanation: vec![dispatcher::DecisionReason(candidate.reason.clone())],
    })
}

pub(crate) fn execution_plan_for_explicit_provider_candidate(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidate: &ProviderCandidate,
) -> Result<dispatcher::ExecutionPlan, std::io::Error> {
    let mut explicit = config.clone();
    if explicit.output_adapter == OutputAdapterPreference::Auto && candidate.rtk.is_some() {
        explicit.output_adapter = OutputAdapterPreference::Rtk;
    }
    execution_plan_for_provider_candidate(tool, arguments, &explicit, candidate)
}

pub(crate) fn first_compatible_provider_plan<'a>(
    tool: &str,
    arguments: &[OsString],
    config: &Config,
    candidates: &'a [ProviderCandidate],
) -> Option<(usize, &'a ProviderCandidate, dispatcher::ExecutionPlan)> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.usable && candidate.has_consistent_location())
        .find_map(|(index, candidate)| {
            execution_plan_for_explicit_provider_candidate(tool, arguments, config, candidate)
                .ok()
                .map(|plan| (index, candidate, plan))
        })
}

pub(crate) fn is_shell_operator_command(arguments: &[OsString]) -> bool {
    matches!(
        arguments.first().and_then(|argument| argument.to_str()),
        Some("|" | "||" | "&&" | ";" | "<" | ">" | ">>")
    )
}
