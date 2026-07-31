use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::metrics::xuva_data_root;
use crate::providers::model::ProviderCacheFile;

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
    let root = xuva_data_root();
    fs::create_dir_all(&root)
        .map_err(|error| format!("unable to create provider cache directory: {error}"))?;
    let target = provider_cache_path();
    let temporary = root.join(format!("provider-cache-{}.pending", std::process::id()));
    let contents = serde_json::to_vec_pretty(cache)
        .map_err(|error| format!("unable to encode provider cache: {error}"))?;
    fs::write(&temporary, contents)
        .map_err(|error| format!("unable to write provider cache: {error}"))?;
    if target.exists() {
        let _ = fs::remove_file(&target);
    }
    fs::rename(&temporary, &target)
        .map_err(|error| format!("unable to finalize provider cache: {error}"))
}
