use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::adapters::rtk::adapter_contract_id;
use crate::config::{PolicyObjective, Route};
use crate::metrics::{TokenTotals, xuva_data_root};
use crate::state;

use super::{
    CALIBRATION_MAX_SAMPLES, CALIBRATION_SCHEMA_VERSION, CalibrationEntry, CalibrationFile,
    CalibrationPlan, MAX_CALIBRATION_ENTRIES, NativeCalibrationSample, valid_context_signature,
    valid_evidence_key,
};

fn path() -> PathBuf {
    xuva_data_root().join("calibration-v3.json")
}

pub(crate) fn validate(file: &CalibrationFile) -> Result<(), String> {
    if file.schema_version != CALIBRATION_SCHEMA_VERSION
        || file.entries.len() > MAX_CALIBRATION_ENTRIES
    {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }
    let mut signatures = HashSet::new();
    for entry in &file.entries {
        if !valid_context_signature(&entry.signature)
            || !valid_evidence_key(&entry.key)
            || entry.manifest_version != adapter_contract_id()
            || !valid_context_signature(&entry.context_signature)
            || entry.raw_samples_ms.len() > CALIBRATION_MAX_SAMPLES
            || entry.native_samples.len() > CALIBRATION_MAX_SAMPLES
            || (entry.raw_samples_ms.is_empty() && entry.native_samples.is_empty())
            || !entry
                .raw_samples_ms
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0)
            || !entry.native_samples.iter().all(|sample| {
                sample.elapsed_ms.is_finite()
                    && sample.elapsed_ms >= 0.0
                    && sample.input_tokens >= 0
                    && sample.saved_tokens >= 0
                    && sample.saved_tokens <= sample.input_tokens
            })
            || !signatures.insert(&entry.signature)
        {
            return Err("calibration state contains invalid local evidence".to_owned());
        }
    }
    Ok(())
}

pub(crate) fn for_current_contract(mut file: CalibrationFile) -> Result<CalibrationFile, String> {
    if file.schema_version < CALIBRATION_SCHEMA_VERSION {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
    if file.schema_version != CALIBRATION_SCHEMA_VERSION {
        return Err("calibration state uses an unsupported schema version".to_owned());
    }

    file.entries
        .retain(|entry| entry.manifest_version == adapter_contract_id());
    validate(&file)?;
    Ok(file)
}

pub(crate) fn load() -> Result<CalibrationFile, String> {
    load_from(&path())
}

fn load_from(path: &Path) -> Result<CalibrationFile, String> {
    if !path.exists() {
        return Ok(CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: Vec::new(),
        });
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("unable to read local calibration state: {error}"))?;
    let file: CalibrationFile = serde_json::from_str(&contents)
        .map_err(|error| format!("local calibration state is invalid: {error}"))?;
    for_current_contract(file)
}

fn cap_samples<T>(samples: &mut Vec<T>) {
    if samples.len() > CALIBRATION_MAX_SAMPLES {
        let excess = samples.len() - CALIBRATION_MAX_SAMPLES;
        samples.drain(0..excess);
    }
}

pub(crate) fn record(
    plan: &CalibrationPlan,
    executed_route: Route,
    elapsed: Duration,
    exit_code: i32,
    totals: TokenTotals,
) -> Result<(), String> {
    if exit_code != 0 || !matches!(executed_route, Route::Raw | Route::NativeRtk) {
        return Ok(());
    }
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let destination = path();
    state::update_json_atomic(
        &destination,
        "local calibration state",
        load_from,
        |state| {
            let entry = match state
                .entries
                .iter()
                .position(|entry| entry.signature == plan.signature)
            {
                Some(index)
                    if state.entries[index].manifest_version == plan.manifest_version
                        && state.entries[index].context_signature == plan.context_signature =>
                {
                    &mut state.entries[index]
                }
                Some(index) => {
                    state.entries[index] = CalibrationEntry {
                        signature: plan.signature.clone(),
                        key: plan.key.clone(),
                        manifest_version: plan.manifest_version.clone(),
                        context_signature: plan.context_signature.clone(),
                        raw_samples_ms: Vec::new(),
                        native_samples: Vec::new(),
                    };
                    &mut state.entries[index]
                }
                None => {
                    state.entries.push(CalibrationEntry {
                        signature: plan.signature.clone(),
                        key: plan.key.clone(),
                        manifest_version: plan.manifest_version.clone(),
                        context_signature: plan.context_signature.clone(),
                        raw_samples_ms: Vec::new(),
                        native_samples: Vec::new(),
                    });
                    state.entries.last_mut().expect("entry was just appended")
                }
            };
            match executed_route {
                Route::Raw => {
                    entry.raw_samples_ms.push(elapsed_ms);
                    cap_samples(&mut entry.raw_samples_ms);
                }
                Route::NativeRtk => {
                    entry.native_samples.push(NativeCalibrationSample {
                        elapsed_ms,
                        input_tokens: totals.input_tokens,
                        saved_tokens: totals.saved_tokens,
                    });
                    cap_samples(&mut entry.native_samples);
                }
                Route::Wsl1 | Route::Wsl2 | Route::Auto => {
                    unreachable!("route was filtered above")
                }
            }
            Ok(())
        },
        validate,
    )
}

pub(crate) fn print(objective: PolicyObjective) -> Result<(), String> {
    let state = load()?;
    if state.entries.is_empty() {
        println!("No local adaptive calibration evidence is recorded.");
        return Ok(());
    }
    println!("XUVA Local Adaptive Calibration");
    println!();
    for entry in &state.entries {
        let route = entry.selected_route(objective);
        println!("key={}", entry.key);
        println!("signature={}", entry.signature);
        println!("phase={}", entry.phase());
        println!("route={}", route.as_str());
        println!("raw_samples={}", entry.raw_samples_ms.len());
        println!("native_samples={}", entry.native_samples.len());
        println!(
            "native_token_savings_percent={:.1}",
            entry.token_savings_percent()
        );
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::routing::{median, select_adaptive_route};

    use super::*;

    fn valid_entry() -> CalibrationEntry {
        CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            manifest_version: adapter_contract_id(),
            context_signature: "fedcba9876543210".to_owned(),
            raw_samples_ms: vec![1.0],
            native_samples: Vec::new(),
        }
    }

    #[test]
    fn validation_rejects_empty_or_oversized_calibration_evidence() {
        let mut empty = valid_entry();
        empty.raw_samples_ms.clear();
        assert!(
            validate(&CalibrationFile {
                schema_version: CALIBRATION_SCHEMA_VERSION,
                entries: vec![empty],
            })
            .is_err()
        );

        let mut oversized = valid_entry();
        oversized.raw_samples_ms = vec![1.0; CALIBRATION_MAX_SAMPLES + 1];
        assert!(
            validate(&CalibrationFile {
                schema_version: CALIBRATION_SCHEMA_VERSION,
                entries: vec![oversized],
            })
            .is_err()
        );

        let mut invalid_signature = valid_entry();
        invalid_signature.signature = "zzzzzzzzzzzzzzzz".to_owned();
        assert!(
            validate(&CalibrationFile {
                schema_version: CALIBRATION_SCHEMA_VERSION,
                entries: vec![invalid_signature],
            })
            .is_err()
        );
    }

    #[test]
    fn stale_adapter_contract_is_discarded_without_failing() {
        let stale = CalibrationFile {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            entries: vec![CalibrationEntry {
                signature: "0123456789abcdef".to_owned(),
                key: "rg".to_owned(),
                manifest_version: "wad:0.42.0:protocol-1".to_owned(),
                context_signature: "fedcba9876543210".to_owned(),
                raw_samples_ms: vec![1.0],
                native_samples: vec![NativeCalibrationSample {
                    elapsed_ms: 2.0,
                    input_tokens: 10,
                    saved_tokens: 5,
                }],
            }],
        };

        let migrated = for_current_contract(stale).expect("stale evidence is safely ignored");
        assert_eq!(migrated.schema_version, CALIBRATION_SCHEMA_VERSION);
        assert!(migrated.entries.is_empty());
    }

    #[test]
    fn measured_token_savings_remain_part_of_route_selection() {
        let entry = CalibrationEntry {
            signature: "0123456789abcdef".to_owned(),
            key: "rg".to_owned(),
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            raw_samples_ms: vec![10.0, 11.0],
            native_samples: vec![
                NativeCalibrationSample {
                    elapsed_ms: 30.0,
                    input_tokens: 50,
                    saved_tokens: 10,
                },
                NativeCalibrationSample {
                    elapsed_ms: 31.0,
                    input_tokens: 50,
                    saved_tokens: 15,
                },
            ],
        };
        assert_eq!(entry.phase(), "stable");
        assert_eq!(
            entry.selected_route(PolicyObjective::Balanced),
            Route::NativeRtk
        );
        assert_eq!(entry.selected_route(PolicyObjective::Latency), Route::Raw);
        assert_eq!(
            select_adaptive_route(Some(10.0), Some(30.0), 1.0, PolicyObjective::Tokens),
            Route::NativeRtk
        );
        assert_eq!(median(&[1.0, 3.0]), Some(2.0));
    }
}
