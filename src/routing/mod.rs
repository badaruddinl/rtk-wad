use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;

use crate::adapters::rtk::adapter_contract_id;
use crate::config::{Config, PolicyObjective, Route};

pub(crate) const ROUTE_POLICY_SCHEMA_VERSION: u32 = 2;
pub(crate) const CALIBRATION_SCHEMA_VERSION: u32 = 3;
pub(crate) const CALIBRATION_MAX_SAMPLES: usize = 5;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RoutePolicyFile {
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) manifest_version: String,
    #[serde(default)]
    pub(crate) context_signature: String,
    pub(crate) evidence: Vec<RoutePolicyEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoutePolicyEvidence {
    pub(crate) key: String,
    pub(crate) raw_median_ms: f64,
    pub(crate) candidate_median_ms: f64,
    pub(crate) token_savings_percent: f64,
    pub(crate) sample_count: u32,
}

#[derive(Serialize)]
pub(crate) struct PolicyContextReport {
    pub(crate) schema_version: u32,
    pub(crate) manifest_version: String,
    pub(crate) context_signature: String,
}

pub(crate) fn policy_context_report(config: &Config) -> PolicyContextReport {
    PolicyContextReport {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: adapter_contract_id(),
        context_signature: adaptive_context_signature(config),
    }
}

impl RoutePolicyFile {
    pub(crate) fn route_for(
        &self,
        key: &str,
        context_signature: &str,
        objective: PolicyObjective,
    ) -> Option<Route> {
        let evidence = self.evidence.iter().find(|evidence| evidence.key == key)?;
        if self.schema_version != ROUTE_POLICY_SCHEMA_VERSION
            || self.manifest_version != adapter_contract_id()
            || self.context_signature != context_signature
            || evidence.sample_count < 5
        {
            return None;
        }
        Some(select_adaptive_route(
            Some(evidence.raw_median_ms),
            Some(evidence.candidate_median_ms),
            evidence.token_savings_percent,
            objective,
        ))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct CalibrationFile {
    pub(crate) schema_version: u32,
    pub(crate) entries: Vec<CalibrationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CalibrationEntry {
    pub(crate) signature: String,
    pub(crate) key: String,
    #[serde(default)]
    pub(crate) manifest_version: String,
    #[serde(default)]
    pub(crate) context_signature: String,
    pub(crate) raw_samples_ms: Vec<f64>,
    pub(crate) native_samples: Vec<NativeCalibrationSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NativeCalibrationSample {
    pub(crate) elapsed_ms: f64,
    pub(crate) input_tokens: i64,
    pub(crate) saved_tokens: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct CalibrationPlan {
    pub(crate) signature: String,
    pub(crate) key: String,
    pub(crate) manifest_version: String,
    pub(crate) context_signature: String,
    pub(crate) route: Route,
    pub(crate) reason: &'static str,
}

impl CalibrationEntry {
    pub(crate) fn token_savings_percent(&self) -> f64 {
        let input_tokens = self
            .native_samples
            .iter()
            .map(|sample| sample.input_tokens)
            .sum::<i64>();
        let saved_tokens = self
            .native_samples
            .iter()
            .map(|sample| sample.saved_tokens)
            .sum::<i64>();
        if input_tokens > 0 {
            (saved_tokens as f64 / input_tokens as f64) * 100.0
        } else {
            0.0
        }
    }

    pub(crate) fn selected_route(&self, objective: PolicyObjective) -> Route {
        select_adaptive_route(
            median(&self.raw_samples_ms),
            median(
                &self
                    .native_samples
                    .iter()
                    .map(|sample| sample.elapsed_ms)
                    .collect::<Vec<_>>(),
            ),
            self.token_savings_percent(),
            objective,
        )
    }

    pub(crate) fn phase(&self) -> &'static str {
        if self.raw_samples_ms.is_empty() || self.native_samples.len() < 2 {
            "candidate"
        } else if self.raw_samples_ms.len() < 2 {
            "provisional"
        } else {
            "stable"
        }
    }
}

pub(crate) fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        Some((sorted[middle - 1] + sorted[middle]) / 2.0)
    } else {
        Some(sorted[middle])
    }
}

pub(crate) fn select_adaptive_route(
    raw_median_ms: Option<f64>,
    native_median_ms: Option<f64>,
    token_savings_percent: f64,
    objective: PolicyObjective,
) -> Route {
    let raw_is_faster = raw_median_ms
        .zip(native_median_ms)
        .is_some_and(|(raw, native)| raw <= native);
    match objective {
        PolicyObjective::Latency => {
            if raw_is_faster {
                Route::Raw
            } else {
                Route::NativeRtk
            }
        }
        PolicyObjective::Balanced => {
            if token_savings_percent >= 25.0 {
                Route::NativeRtk
            } else if raw_is_faster {
                Route::Raw
            } else {
                Route::NativeRtk
            }
        }
        PolicyObjective::Tokens => {
            if token_savings_percent > 0.0 {
                Route::NativeRtk
            } else if raw_is_faster {
                Route::Raw
            } else {
                Route::NativeRtk
            }
        }
    }
}

pub(crate) fn adaptive_context_signature(config: &Config) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut append = |value: &str| {
        for byte in value.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    append(&adapter_contract_id());
    append(config.environment.as_str());
    append(config.policy_objective.as_str());
    append(&config.native_rtk_path);
    append(&env::var_os("PATH").unwrap_or_default().to_string_lossy());
    format!("{hash:016x}")
}

pub(crate) fn calibration_signature(
    arguments: &[OsString],
    current_directory: Option<&str>,
) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut append = |value: &str| {
        for byte in value.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    append(current_directory.unwrap_or_default());
    for argument in arguments {
        append(&argument.to_string_lossy());
    }
    format!("{hash:016x}")
}

pub(crate) fn calibration_plan(
    arguments: &[OsString],
    current_directory: Option<&str>,
    policy: Option<&RoutePolicyFile>,
    context_signature: &str,
    objective: PolicyObjective,
) -> Result<Option<CalibrationPlan>, String> {
    let Some(current_directory) = current_directory else {
        return Ok(None);
    };
    let Some(command) = arguments.first() else {
        return Ok(None);
    };
    let command_str = command.to_string_lossy();
    let signature = calibration_signature(arguments, Some(current_directory));
    let key = command_str.to_string();

    if let Some(route) = policy.and_then(|p| p.route_for(&key, context_signature, objective)) {
        return Ok(Some(CalibrationPlan {
            signature,
            key,
            manifest_version: adapter_contract_id(),
            context_signature: context_signature.to_string(),
            route,
            reason: "stable adaptive policy evidence matched current context",
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests;
