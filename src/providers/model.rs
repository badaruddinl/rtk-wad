use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProjectLocationKind {
    Windows,
    Wsl,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ProjectLocation {
    pub(crate) kind: ProjectLocationKind,
    pub(crate) path: String,
    pub(crate) distro: Option<String>,
    pub(crate) windows_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WindowsToolProbe {
    pub(crate) executable: Option<String>,
    pub(crate) native_rtk: Option<String>,
    #[serde(default)]
    pub(crate) executable_version: Option<String>,
    #[serde(default)]
    pub(crate) version_probe_status: ProbeStatus,
    #[serde(default)]
    pub(crate) executable_capabilities: Vec<String>,
    #[serde(default)]
    pub(crate) executable_identity: Option<BinaryIdentity>,
    #[serde(default)]
    pub(crate) native_rtk_identity: Option<BinaryIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WslToolProbe {
    pub(crate) distro: String,
    #[serde(default)]
    pub(crate) user: Option<String>,
    pub(crate) wsl_version: Option<u8>,
    #[serde(default)]
    pub(crate) dedicated: bool,
    #[serde(default)]
    pub(crate) installation_id: Option<String>,
    pub(crate) executable: Option<String>,
    pub(crate) rtk: Option<String>,
    #[serde(default)]
    pub(crate) executable_version: Option<String>,
    #[serde(default)]
    pub(crate) version_probe_status: ProbeStatus,
    #[serde(default)]
    pub(crate) executable_capabilities: Vec<String>,
    #[serde(default)]
    pub(crate) executable_identity: Option<BinaryIdentity>,
    #[serde(default)]
    pub(crate) rtk_identity: Option<BinaryIdentity>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProbeStatus {
    #[default]
    NotRequested,
    NotSupported,
    Success,
    Failed,
    Timeout,
    OutputLimit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct BinaryIdentity {
    pub(crate) path: String,
    pub(crate) file_key: String,
    pub(crate) size_bytes: u64,
    pub(crate) modified_stamp: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InspectionLevel {
    #[default]
    Identity,
    Version,
    Capability,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ProviderCacheEntry {
    pub(crate) tool: String,
    pub(crate) observed_unix_seconds: u64,
    #[serde(default)]
    pub(crate) inspection_level: InspectionLevel,
    #[serde(default)]
    pub(crate) context_signature: String,
    pub(crate) windows: WindowsToolProbe,
    #[serde(default)]
    pub(crate) wsl_probe_complete: bool,
    pub(crate) wsl: Vec<WslToolProbe>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ProviderCacheFile {
    pub(crate) schema_version: u32,
    pub(crate) entries: Vec<ProviderCacheEntry>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderHost {
    Windows,
    Wsl1,
    Wsl2,
}

impl ProviderHost {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Wsl1 => "wsl1",
            Self::Wsl2 => "wsl2",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdapterKind {
    Raw,
    Rtk,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ProviderCandidate {
    pub(crate) host: ProviderHost,
    pub(crate) adapters: Vec<AdapterKind>,
    pub(crate) distro: Option<String>,
    pub(crate) wsl_version: Option<u8>,
    pub(crate) executable: String,
    pub(crate) executable_identity: Option<BinaryIdentity>,
    pub(crate) rtk: Option<String>,
    pub(crate) rtk_identity: Option<BinaryIdentity>,
    pub(crate) project_path: Option<String>,
    pub(crate) usable: bool,
    pub(crate) reason: String,
}

impl ProviderCandidate {
    pub(crate) fn supports_adapter(&self, adapter: AdapterKind) -> bool {
        self.adapters.contains(&adapter)
    }

    pub(crate) fn is_windows(&self) -> bool {
        self.host == ProviderHost::Windows
    }

    pub(crate) fn is_wsl(&self) -> bool {
        matches!(self.host, ProviderHost::Wsl1 | ProviderHost::Wsl2)
    }

    pub(crate) fn has_consistent_location(&self) -> bool {
        match self.host {
            ProviderHost::Windows => self.distro.is_none() && self.wsl_version.is_none(),
            ProviderHost::Wsl1 => self.distro.is_some() && self.wsl_version == Some(1),
            ProviderHost::Wsl2 => self.distro.is_some() && self.wsl_version == Some(2),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProviderResolution {
    pub(crate) schema_version: u32,
    pub(crate) tool: String,
    pub(crate) cache: &'static str,
    pub(crate) project: ProjectLocation,
    pub(crate) availability: ProviderCacheEntry,
    pub(crate) candidates: Vec<ProviderCandidate>,
    pub(crate) recommended: Option<usize>,
    pub(crate) diagnosis: String,
    pub(crate) install: &'static str,
}
