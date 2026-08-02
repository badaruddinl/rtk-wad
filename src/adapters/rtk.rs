//! Versioned RTK adapter protocol and command vocabulary.
//!
//! The XUVA core consumes this adapter contract without hard-coding RTK's
//! command list in routing code. Updating RTK therefore changes this module and
//! its manifest contract, not the generic provider/execution abstractions.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const COMMAND_MANIFEST: &str = include_str!("../../benchmarks/command-manifest.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandManifest {
    pub(crate) schema_version: u32,
    pub(crate) adapter: AdapterContract,
    native_structured: Vec<String>,
    raw_native: Vec<String>,
    raw_mutation_subcommands: BTreeMap<String, Vec<String>>,
    raw_read_only_subcommands: BTreeMap<String, Vec<String>>,
    wsl1_conservative: Vec<String>,
    core_internal: Vec<String>,
    source: String,
    coverage_rule: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdapterContract {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) compatible_versions: Vec<String>,
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
        let manifest: CommandManifest = serde_json::from_str(COMMAND_MANIFEST)
            .expect("embedded command manifest must be valid JSON");
        validate_manifest(&manifest).expect("embedded command manifest must be internally valid");
        manifest
    })
}

fn validate_manifest(manifest: &CommandManifest) -> Result<(), String> {
    let adapter = &manifest.adapter;
    if manifest.schema_version != 3
        || adapter.name.is_empty()
        || manifest.source.trim().is_empty()
        || manifest.coverage_rule.trim().is_empty()
        || adapter.protocol_version == 0
        || adapter.compatible_versions.is_empty()
        || adapter.compatible_versions.len() > 16
        || !adapter
            .compatible_versions
            .iter()
            .any(|version| version == &adapter.version)
        || adapter.compatible_versions.iter().any(|version| {
            version.is_empty()
                || version.len() > 32
                || !version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
        })
    {
        return Err("adapter identity or compatibility allowlist is invalid".to_owned());
    }
    let versions = adapter
        .compatible_versions
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if versions.len() != adapter.compatible_versions.len() {
        return Err("adapter compatibility allowlist contains duplicates".to_owned());
    }
    for (command, read_only) in &manifest.raw_read_only_subcommands {
        let Some(mutations) = manifest.raw_mutation_subcommands.get(command) else {
            return Err(format!("{command} has no mutation contract"));
        };
        if !manifest
            .native_structured
            .iter()
            .any(|item| item == command)
            || read_only.is_empty()
            || mutations.is_empty()
            || read_only.iter().chain(mutations).any(|subcommand| {
                subcommand.is_empty()
                    || subcommand.len() > 64
                    || !subcommand.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            })
        {
            return Err(format!("{command} has an invalid subcommand contract"));
        }
        let read_only = read_only.iter().collect::<std::collections::HashSet<_>>();
        if mutations.iter().any(|item| read_only.contains(item)) {
            return Err(format!("{command} subcommand contracts overlap"));
        }
    }
    if manifest
        .raw_mutation_subcommands
        .keys()
        .any(|command| !manifest.raw_read_only_subcommands.contains_key(command))
    {
        return Err("mutation contract has no matching read-only contract".to_owned());
    }
    Ok(())
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
    name.eq_ignore_ascii_case(&adapter.name)
        && adapter
            .compatible_versions
            .iter()
            .any(|compatible| version.trim_start_matches('v') == compatible)
}

pub(crate) fn is_read_only_subcommand(command: &str, subcommand: &str) -> bool {
    command_manifest()
        .raw_read_only_subcommands
        .get(command)
        .is_some_and(|subcommands| subcommands.iter().any(|item| item == subcommand))
}

pub(crate) fn is_mutation_subcommand(command: &str, subcommand: &str) -> bool {
    command_manifest()
        .raw_mutation_subcommands
        .get(command)
        .is_some_and(|subcommands| subcommands.iter().any(|item| item == subcommand))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        adapter_version_is_compatible, command_manifest, is_mutation_subcommand,
        is_read_only_subcommand,
    };

    #[test]
    fn adapter_identity_requires_the_manifest_name_and_reviewed_version_allowlist() {
        assert!(adapter_version_is_compatible(Some("rtk 0.43.0")));
        assert!(adapter_version_is_compatible(Some("RTK v0.43.0")));
        assert!(!adapter_version_is_compatible(Some("rtk 0.42.0")));
        assert!(!adapter_version_is_compatible(Some("other 0.43.0")));
        assert!(!adapter_version_is_compatible(Some("0.43.0")));
        assert!(!adapter_version_is_compatible(None));
    }

    #[test]
    fn subcommand_contract_is_complete_non_overlapping_and_runtime_backed() {
        let manifest = command_manifest();
        let read_only = manifest
            .raw_read_only_subcommands
            .get("git")
            .expect("Git read-only contract is declared");
        let mutations = manifest
            .raw_mutation_subcommands
            .get("git")
            .expect("Git mutation contract is declared");
        let read_only_set = read_only.iter().collect::<HashSet<_>>();
        assert!(mutations.iter().all(|item| !read_only_set.contains(item)));
        assert!(
            read_only
                .iter()
                .all(|item| is_read_only_subcommand("git", item))
        );
        assert!(
            mutations
                .iter()
                .all(|item| is_mutation_subcommand("git", item))
        );
        assert!(!is_read_only_subcommand("git", "future-command"));
        assert!(!is_mutation_subcommand("git", "future-command"));
    }
}
