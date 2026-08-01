use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::metrics::xuva_data_root;
use crate::providers::model::{InspectionLevel, ProviderCacheEntry, ProviderCacheFile};
use crate::state::write_json_atomic;

pub(crate) const PROVIDER_CACHE_SCHEMA_VERSION: u32 = 5;
pub(crate) const PROVIDER_CACHE_TTL_SECONDS: u64 = 300;

pub(crate) fn provider_cache_path() -> PathBuf {
    xuva_data_root().join("provider-cache-v5.json")
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub(crate) fn load_provider_cache() -> ProviderCacheFile {
    fs::read_to_string(provider_cache_path())
        .ok()
        .and_then(|contents| serde_json::from_str::<ProviderCacheFile>(&contents).ok())
        .filter(|cache| cache.schema_version == PROVIDER_CACHE_SCHEMA_VERSION)
        .unwrap_or(ProviderCacheFile {
            schema_version: PROVIDER_CACHE_SCHEMA_VERSION,
            entries: Vec::new(),
        })
}

pub(crate) fn save_provider_cache(cache: &ProviderCacheFile) -> Result<(), String> {
    write_json_atomic(&provider_cache_path(), cache, "provider cache")
}

pub(crate) fn cache_entry_is_fresh(
    entry: &ProviderCacheEntry,
    now: u64,
    context_signature: &str,
    validate_versions: bool,
) -> bool {
    let required_level = if validate_versions {
        InspectionLevel::Version
    } else {
        InspectionLevel::Identity
    };
    now.saturating_sub(entry.observed_unix_seconds) <= PROVIDER_CACHE_TTL_SECONDS
        && entry.context_signature == context_signature
        && entry.inspection_level >= required_level
}

pub(crate) fn discovery_context_signature(config: &Config, require_wsl: bool) -> String {
    let path_value = std::env::var_os("PATH").unwrap_or_default();
    let path_ext_value = std::env::var_os("PATHEXT").unwrap_or_default();
    let path = path_value.to_string_lossy();
    let path_ext = path_ext_value.to_string_lossy();
    let configured = format!(
        "{}:{}:{}:{}:{}",
        config.distro,
        config.user.as_deref().unwrap_or_default(),
        config.native_rtk_path,
        config.extra_path.as_deref().unwrap_or_default(),
        if require_wsl {
            "wsl-inventory-required"
        } else {
            "windows-only"
        },
    );
    stable_signature(&[&path, &path_ext, &configured])
}

fn stable_signature(parts: &[&str]) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0xff)) {
            state ^= u64::from(byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{state:016x}")
}
