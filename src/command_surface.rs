use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const COMMAND_MANIFEST: &str = include_str!("../benchmarks/command-manifest.json");

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CommandManifest {
    pub(crate) schema_version: u32,
    pub(crate) upstream_rtk_version: String,
    native_structured: Vec<String>,
    raw_native: Vec<String>,
    wsl1_conservative: Vec<String>,
    wad_internal: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandSurface {
    NativeStructured,
    RawNative,
    Wsl1Conservative,
    WadInternal,
    Unknown,
}

impl CommandSurface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NativeStructured => "native-structured",
            Self::RawNative => "raw-native",
            Self::Wsl1Conservative => "wsl1-conservative",
            Self::WadInternal => "wad-internal",
            Self::Unknown => "unknown",
        }
    }

    fn default_route(self) -> &'static str {
        match self {
            Self::NativeStructured => "native-rtk",
            Self::RawNative => "raw",
            Self::Wsl1Conservative | Self::Unknown => "wsl1",
            Self::WadInternal => "internal",
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
    pub(crate) upstream_rtk_version: String,
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
    } else if manifest.wad_internal.iter().any(|item| item == command) {
        CommandSurface::WadInternal
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
        .chain(&manifest.wad_internal)
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
        upstream_rtk_version: manifest.upstream_rtk_version.clone(),
        upstream_command_count: rows.len(),
        commands: rows,
    }
}
