//! Versioned RTK adapter protocol and command vocabulary.
//!
//! The XUVA core consumes this adapter contract without hard-coding RTK's
//! command list in routing code. Updating RTK therefore changes this module and
//! its manifest contract, not the generic provider/execution abstractions.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const COMMAND_MANIFEST: &str = include_str!("../../benchmarks/command-manifest.json");

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CommandManifest {
    pub(crate) schema_version: u32,
    pub(crate) adapter: AdapterContract,
    native_structured: Vec<String>,
    raw_native: Vec<String>,
    wsl1_conservative: Vec<String>,
    core_internal: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AdapterContract {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) protocol_version: u32,
    pub(crate) capabilities: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandSurface {
    NativeStructured,
    RawNative,
    Wsl1Conservative,
    CoreInternal,
    Unknown,
}

impl CommandSurface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NativeStructured => "native-structured",
            Self::RawNative => "raw-native",
            Self::Wsl1Conservative => "wsl1-conservative",
            Self::CoreInternal => "core-internal",
            Self::Unknown => "unknown",
        }
    }

    fn default_route(self) -> &'static str {
        match self {
            Self::NativeStructured => "native-rtk",
            Self::RawNative => "raw",
            Self::Wsl1Conservative | Self::Unknown => "wsl1",
            Self::CoreInternal => "internal",
        }
    }
}

#[derive(Serialize)]
pub(crate) struct CommandSurfaceRow {
    pub(crate) command: String,
    pub(crate) classification: CommandSurface,
    pub(crate) default_route: &'static str,
}

#[derive(Serialize)]
pub(crate) struct CommandSurfaceReport {
    pub(crate) schema_version: u32,
    pub(crate) adapter: AdapterContract,
    pub(crate) upstream_command_count: usize,
    pub(crate) commands: Vec<CommandSurfaceRow>,
}

pub(crate) fn command_manifest() -> &'static CommandManifest {
    static PARSED: OnceLock<CommandManifest> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(COMMAND_MANIFEST)
            .expect("embedded command manifest must be valid JSON")
    })
}

pub(crate) fn command_surface(command: &str) -> CommandSurface {
    let manifest = command_manifest();
    if manifest
        .native_structured
        .iter()
        .any(|item| item == command)
    {
        CommandSurface::NativeStructured
    } else if manifest.raw_native.iter().any(|item| item == command) {
        CommandSurface::RawNative
    } else if manifest
        .wsl1_conservative
        .iter()
        .any(|item| item == command)
    {
        CommandSurface::Wsl1Conservative
    } else if manifest.core_internal.iter().any(|item| item == command) {
        CommandSurface::CoreInternal
    } else {
        CommandSurface::Unknown
    }
}

pub(crate) fn command_surface_report() -> CommandSurfaceReport {
    let manifest = command_manifest();
    let mut commands = manifest
        .native_structured
        .iter()
        .chain(&manifest.raw_native)
        .chain(&manifest.wsl1_conservative)
        .chain(&manifest.core_internal)
        .filter(|command| !command.starts_with('-') && command.as_str() != "stats")
        .cloned()
        .collect::<Vec<_>>();
    commands.sort();
    commands.dedup();
    let rows = commands
        .into_iter()
        .map(|command| {
            let classification = command_surface(&command);
            CommandSurfaceRow {
                default_route: classification.default_route(),
                command,
                classification,
            }
        })
        .collect::<Vec<_>>();
    CommandSurfaceReport {
        schema_version: manifest.schema_version,
        adapter: manifest.adapter.clone(),
        upstream_command_count: rows.len(),
        commands: rows,
    }
}

pub(crate) fn adapter_contract_id() -> String {
    let adapter = &command_manifest().adapter;
    format!(
        "{}:{}:protocol-{}",
        adapter.name, adapter.version, adapter.protocol_version
    )
}

pub(crate) fn adapter_version_is_compatible(observed: Option<&str>) -> bool {
    let Some(observed) = observed else {
        return false;
    };
    let mut fields = observed.split_whitespace();
    let Some(name) = fields.next() else {
        return false;
    };
    let Some(version) = fields.next() else {
        return false;
    };
    let adapter = &command_manifest().adapter;
    name.eq_ignore_ascii_case(&adapter.name) && version.trim_start_matches('v') == adapter.version
}

#[cfg(test)]
mod tests {
    use super::adapter_version_is_compatible;

    #[test]
    fn adapter_identity_requires_the_manifest_name_and_exact_version() {
        assert!(adapter_version_is_compatible(Some("rtk 0.43.0")));
        assert!(adapter_version_is_compatible(Some("RTK v0.43.0")));
        assert!(!adapter_version_is_compatible(Some("rtk 0.42.0")));
        assert!(!adapter_version_is_compatible(Some("other 0.43.0")));
        assert!(!adapter_version_is_compatible(Some("0.43.0")));
        assert!(!adapter_version_is_compatible(None));
    }
}
