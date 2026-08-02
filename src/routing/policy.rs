use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::adapters::rtk::adapter_contract_id;
use crate::config::Config;
use crate::metrics::xuva_data_root;
use crate::state;

use super::{
    MAX_POLICY_EVIDENCE, ROUTE_POLICY_SCHEMA_VERSION, RoutePolicyFile, adaptive_context_signature,
    valid_context_signature,
};

fn policy_path() -> PathBuf {
    env::var_os("XUVA_POLICY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| xuva_data_root().join("route-policy-v2.json"))
}

pub(crate) fn load() -> Option<RoutePolicyFile> {
    let contents = fs::read_to_string(policy_path()).ok()?;
    let policy = serde_json::from_str(&contents).ok()?;
    validate(&policy).ok()?;
    Some(policy)
}

pub(crate) fn validate(policy: &RoutePolicyFile) -> Result<(), String> {
    if policy.schema_version != ROUTE_POLICY_SCHEMA_VERSION
        || policy.manifest_version != adapter_contract_id()
        || !valid_context_signature(&policy.context_signature)
        || policy.evidence.is_empty()
        || policy.evidence.len() > MAX_POLICY_EVIDENCE
    {
        return Err(
            "policy evidence must use the current schema, manifest, context, and non-empty evidence"
                .to_owned(),
        );
    }
    let mut keys = HashSet::new();
    for evidence in &policy.evidence {
        if !evidence.is_valid() {
            return Err(format!(
                "policy evidence `{}` contains an invalid key, sample count, duration, or token-savings percentage",
                evidence.key
            ));
        }
        if !keys.insert(&evidence.key) {
            return Err(format!(
                "policy evidence contains duplicate key {}",
                evidence.key
            ));
        }
    }
    Ok(())
}

pub(crate) fn merge(
    existing: Option<RoutePolicyFile>,
    incoming: RoutePolicyFile,
) -> RoutePolicyFile {
    let RoutePolicyFile {
        manifest_version,
        context_signature,
        evidence: incoming_evidence,
        ..
    } = incoming;
    let mut evidence = existing.map_or_else(Vec::new, |policy| policy.evidence);
    for next in incoming_evidence {
        if let Some(index) = evidence.iter().position(|current| current.key == next.key) {
            evidence[index] = next;
        } else {
            evidence.push(next);
        }
    }
    evidence.sort_by(|left, right| left.key.cmp(&right.key));
    RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version,
        context_signature,
        evidence,
    }
}

pub(crate) fn import(source: &Path, config: &Config) -> Result<(), String> {
    let contents = fs::read_to_string(source)
        .map_err(|error| format!("unable to read policy evidence: {error}"))?;
    let incoming: RoutePolicyFile = serde_json::from_str(&contents)
        .map_err(|error| format!("invalid policy evidence: {error}"))?;
    validate(&incoming)?;
    let expected_context = adaptive_context_signature(config);
    if incoming.context_signature != expected_context {
        return Err("policy evidence was measured for a different local adapter context; run `xuva policy context` and re-benchmark".to_owned());
    }
    let destination = policy_path();
    let existing = if destination.exists() {
        let contents = fs::read_to_string(&destination)
            .map_err(|error| format!("unable to read existing route policy: {error}"))?;
        let policy = serde_json::from_str(&contents)
            .map_err(|error| format!("existing route policy is invalid: {error}"))?;
        validate(&policy).map_err(|error| format!("existing route policy is invalid: {error}"))?;
        if policy.context_signature != incoming.context_signature {
            return Err("existing policy belongs to a different local adapter context; remove or relocate it before importing new evidence".to_owned());
        }
        Some(policy)
    } else {
        None
    };
    state::write_json_atomic(&destination, &merge(existing, incoming), "route policy")
}

#[cfg(test)]
mod tests {
    use crate::adapters::rtk::adapter_contract_id;
    use crate::config::{PolicyObjective, Route};

    use super::*;
    use crate::routing::{MAX_POLICY_SAMPLE_COUNT, MIN_POLICY_SAMPLE_COUNT, RoutePolicyEvidence};

    fn valid_evidence() -> RoutePolicyEvidence {
        RoutePolicyEvidence {
            key: "rg".to_owned(),
            raw_median_ms: 10.0,
            candidate_median_ms: 20.0,
            token_savings_percent: 25.0,
            sample_count: MIN_POLICY_SAMPLE_COUNT,
        }
    }

    fn policy_with(evidence: RoutePolicyEvidence) -> RoutePolicyFile {
        RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            evidence: vec![evidence],
        }
    }

    #[test]
    fn validation_enforces_policy_measurement_bounds() {
        for savings in [-100.0, 0.0, 100.0] {
            let mut evidence = valid_evidence();
            evidence.token_savings_percent = savings;
            validate(&policy_with(evidence)).expect("bounded expansion or savings is valid");
        }

        for invalid_savings in [-100.1, 100.1, f64::NAN, f64::INFINITY] {
            let mut evidence = valid_evidence();
            evidence.token_savings_percent = invalid_savings;
            assert!(validate(&policy_with(evidence)).is_err());
        }
        for invalid_samples in [0, MIN_POLICY_SAMPLE_COUNT - 1, MAX_POLICY_SAMPLE_COUNT + 1] {
            let mut evidence = valid_evidence();
            evidence.sample_count = invalid_samples;
            assert!(validate(&policy_with(evidence)).is_err());
        }

        let mut invalid_key = valid_evidence();
        invalid_key.key = "rg\nmalformed".to_owned();
        assert!(validate(&policy_with(invalid_key)).is_err());
        let mut invalid_context = policy_with(valid_evidence());
        invalid_context.context_signature = "not-hex-signatur".to_owned();
        assert!(validate(&invalid_context).is_err());
    }

    #[test]
    fn import_merge_preserves_other_evidence_and_replaces_same_key() {
        let existing = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "cargo:check".to_owned(),
                    raw_median_ms: 10.0,
                    candidate_median_ms: 20.0,
                    token_savings_percent: 1.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 20.0,
                    candidate_median_ms: 30.0,
                    token_savings_percent: 80.0,
                    sample_count: 5,
                },
            ],
        };
        let incoming = RoutePolicyFile {
            schema_version: ROUTE_POLICY_SCHEMA_VERSION,
            manifest_version: adapter_contract_id(),
            context_signature: "0123456789abcdef".to_owned(),
            evidence: vec![
                RoutePolicyEvidence {
                    key: "npm:run-list".to_owned(),
                    raw_median_ms: 30.0,
                    candidate_median_ms: 40.0,
                    token_savings_percent: 0.0,
                    sample_count: 5,
                },
                RoutePolicyEvidence {
                    key: "rg".to_owned(),
                    raw_median_ms: 5.0,
                    candidate_median_ms: 10.0,
                    token_savings_percent: 90.0,
                    sample_count: 5,
                },
            ],
        };
        let merged = merge(Some(existing), incoming);
        assert_eq!(
            merged
                .evidence
                .iter()
                .map(|evidence| evidence.key.as_str())
                .collect::<Vec<_>>(),
            vec!["cargo:check", "npm:run-list", "rg"]
        );
        let rg = merged
            .evidence
            .iter()
            .find(|evidence| evidence.key == "rg")
            .expect("new measurement replaces rg");
        assert_eq!(rg.token_savings_percent, 90.0);
        assert_eq!(
            merged.route_for("cargo:check", "0123456789abcdef", PolicyObjective::Balanced,),
            Some(Route::Raw)
        );
        assert_eq!(
            merged.route_for(
                "npm:run-list",
                "0123456789abcdef",
                PolicyObjective::Balanced,
            ),
            Some(Route::Raw)
        );
    }
}
