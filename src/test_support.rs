pub(crate) use std::collections::HashSet;
pub(crate) use std::env;
pub(crate) use std::ffi::OsString;
pub(crate) use std::fs;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::process::Command;

pub(crate) use crate::adapters::rtk::{
    CommandSurface, adapter_contract_id, command_surface, command_surface_report,
};
pub(crate) use crate::adapters::windows::apply_command_spec;
pub(crate) use crate::bridge::{decode_wsl_bridge_fields, wsl_bridge_request};
pub(crate) use crate::cli::{is_verbose_version_command, is_version_command, parse_options};
pub(crate) use crate::config::{
    Config, DEFAULT_DISTRO, DEFAULT_WSL1_DISTRO, ExecutableProfile, ExecutionEnvironment,
    InvocationOrigin, OutputAdapterPreference, PolicyObjective, Route, WslBackend,
};
pub(crate) use crate::dispatcher;
pub(crate) use crate::execution::planner::{
    execution_plan_for_provider_candidate, first_compatible_provider_plan,
    is_shell_operator_command, provider_adapter,
};
pub(crate) use crate::execution::runner::execution_route;
pub(crate) use crate::paths::windows_path_to_wsl_path;
pub(crate) use crate::planning::{
    classify_project_path, provider_environment_policy, windows_cwd_for_invocation,
};
pub(crate) use crate::providers::cache::{
    PROVIDER_CACHE_SCHEMA_VERSION, PROVIDER_CACHE_TTL_SECONDS, cache_entry_is_fresh,
    discovery_context_signature, unix_seconds,
};
pub(crate) use crate::providers::commands::is_safe_provider_tool_name;
pub(crate) use crate::providers::discovery::{
    decode_wsl_output, is_eligible_wsl_distro, is_windows_launchable_path,
    parse_wsl_binary_identity, parse_wsl_distributions, select_windows_executable,
    version_probe_arguments,
};
pub(crate) use crate::providers::dispatch::{
    ProviderDispatchDecision, explicit_executable_plan, is_dispatchable_provider_tool,
    provider_dispatch_decision, provider_dispatch_decision_from_resolution, windows_tool_is_usable,
};
pub(crate) use crate::providers::mapping::{
    windows_mapping_arguments_with_user, windows_project_path_with,
    wsl_mapping_arguments_with_user, wsl_project_path_with,
};
pub(crate) use crate::providers::model::{
    AdapterKind, InspectionLevel, ProbeStatus, ProjectLocation, ProjectLocationKind,
    ProviderCacheEntry, ProviderCandidate, ProviderHost, ProviderResolution, WindowsToolProbe,
    WslToolProbe,
};
pub(crate) use crate::providers::probe::verified_wsl_executable_path;
pub(crate) use crate::providers::resolution::{
    requires_raw_posix_provider, resolve_tool_provider_from_discovery_with_user,
    windows_provider_has_compatible_semantics,
};
pub(crate) use crate::routing::decision::{
    auto_route, auto_route_for_environment, auto_route_with_context, is_adapter_only_rtk_command,
    is_rtk_meta_command, should_use_native_git,
};
pub(crate) use crate::routing::{
    ROUTE_POLICY_SCHEMA_VERSION, RoutePolicyEvidence, RoutePolicyFile, adaptive_context_signature,
    calibration_signature,
};
pub(crate) use crate::self_update::{
    latest_release_from_ls_remote, parsed_stable_version, stable_release_is_newer,
};
pub(crate) use crate::setup::has_complete_go_provider;
pub(crate) use crate::wsl::arguments::{
    LAUNCH_SCRIPT, PLAN_LAUNCH_SCRIPT, WSL1_LAUNCH_SCRIPT, WSL1_MARKER_VALIDATOR_SCRIPT,
    WslLaunchMetadata, plan_wsl_arguments_with_metrics, rtk_arguments, wsl_environment_assignments,
    wsl1_rtk_arguments, wsl1_rtk_arguments_with_metrics,
};
pub(crate) use crate::wsl::authorization::{
    LaunchPermitGuard, verify_pre_authorization_proxy_status,
};
pub(crate) use crate::wsl::cancellation::{CANCEL_SCRIPT, cancel_arguments};

pub(crate) const VERSION_ARGUMENT: &str = "--version";

pub(crate) fn default_config() -> Config {
    Config::from_lookup(|_| None).expect("default config is valid")
}

pub(crate) fn distro_version_from_list(output: &str, distro: &str) -> Option<u8> {
    output.lines().find_map(|line| {
        let trimmed = line.trim().trim_start_matches('*').trim_start();
        let remainder = trimmed.strip_prefix(distro)?;
        if remainder.is_empty() || !remainder.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }
        remainder.split_whitespace().last()?.parse::<u8>().ok()
    })
}
