use std::ffi::OsString;

use crate::adapters::rtk::adapter_contract_id;
use crate::config::{Config, PolicyObjective, Route};
use crate::routing::{
    CalibrationEntry, CalibrationFile, NativeCalibrationSample, ROUTE_POLICY_SCHEMA_VERSION,
    RoutePolicyEvidence, RoutePolicyFile, adaptive_context_signature, calibration_plan,
    calibration_signature, is_calibration_candidate, median, select_adaptive_route,
};

fn default_config() -> Config {
    Config::from_lookup(|_| None).expect("default config is valid")
}

#[test]
fn test_median_calculation() {
    assert_eq!(median(&[]), None);
    assert_eq!(median(&[10.0]), Some(10.0));
    assert_eq!(median(&[10.0, 30.0, 20.0]), Some(20.0));
    assert_eq!(median(&[10.0, 30.0, 20.0, 40.0]), Some(25.0));
}

#[test]
fn test_adaptive_route_selection() {
    assert_eq!(
        select_adaptive_route(Some(10.0), Some(20.0), 30.0, PolicyObjective::Latency),
        Route::Raw
    );
    assert_eq!(
        select_adaptive_route(Some(20.0), Some(10.0), 10.0, PolicyObjective::Latency),
        Route::NativeRtk
    );
    assert_eq!(
        select_adaptive_route(Some(10.0), Some(20.0), 30.0, PolicyObjective::Balanced),
        Route::NativeRtk
    );
    assert_eq!(
        select_adaptive_route(Some(10.0), Some(20.0), 10.0, PolicyObjective::Balanced),
        Route::Raw
    );
    assert_eq!(
        select_adaptive_route(Some(10.0), Some(20.0), 5.0, PolicyObjective::Tokens),
        Route::NativeRtk
    );
}

#[test]
fn test_calibration_entry_route_selection() {
    let entry = CalibrationEntry {
        signature: "test_sig".to_owned(),
        key: "git:status".to_owned(),
        manifest_version: adapter_contract_id(),
        context_signature: "ctx_sig".to_owned(),
        raw_samples_ms: vec![10.0, 12.0],
        native_samples: vec![
            NativeCalibrationSample {
                elapsed_ms: 15.0,
                input_tokens: 100,
                saved_tokens: 30,
            },
            NativeCalibrationSample {
                elapsed_ms: 16.0,
                input_tokens: 100,
                saved_tokens: 30,
            },
        ],
    };
    assert_eq!(entry.token_savings_percent(), 30.0);
    assert_eq!(
        entry.selected_route(PolicyObjective::Balanced),
        Route::NativeRtk
    );
    assert_eq!(entry.phase(), "stable");
}

#[test]
fn calibration_plan_bootstraps_only_fail_closed_safe_commands() {
    let arguments = vec![OsString::from("git"), OsString::from("status")];
    let context = "0123456789abcdef";
    let first = calibration_plan(
        &arguments,
        Some("C:\\repo"),
        None,
        None,
        context,
        PolicyObjective::Balanced,
    )
    .expect("calibration planning succeeds")
    .expect("read-only Git is eligible");
    assert_eq!(first.route, Route::NativeRtk);

    let state = CalibrationFile {
        schema_version: super::CALIBRATION_SCHEMA_VERSION,
        entries: vec![CalibrationEntry {
            signature: first.signature.clone(),
            key: first.key.clone(),
            manifest_version: first.manifest_version.clone(),
            context_signature: context.to_owned(),
            raw_samples_ms: Vec::new(),
            native_samples: vec![NativeCalibrationSample {
                elapsed_ms: 10.0,
                input_tokens: 0,
                saved_tokens: 0,
            }],
        }],
    };
    let second = calibration_plan(
        &arguments,
        Some("C:\\repo"),
        None,
        Some(&state),
        context,
        PolicyObjective::Balanced,
    )
    .expect("calibration planning succeeds")
    .expect("read-only Git remains eligible");
    assert_eq!(second.route, Route::Raw);

    let mutation = [OsString::from("git"), OsString::from("commit")];
    assert!(
        calibration_plan(
            &mutation,
            Some("C:\\repo"),
            None,
            None,
            context,
            PolicyObjective::Balanced,
        )
        .expect("mutation classification succeeds")
        .is_none(),
        "Git mutations must never enter adaptive calibration"
    );
}

#[test]
fn calibration_candidate_check_is_pure_and_fail_closed() {
    for arguments in [
        vec![OsString::from("git"), OsString::from("status")],
        vec![OsString::from("rg"), OsString::from("needle")],
        vec![OsString::from("npm"), OsString::from("run")],
        vec![
            OsString::from("go"),
            OsString::from("test"),
            OsString::from("./..."),
        ],
    ] {
        assert!(is_calibration_candidate(&arguments));
    }

    for arguments in [
        vec![OsString::from("git"), OsString::from("commit")],
        vec![OsString::from("npm"), OsString::from("install")],
        vec![OsString::from("go"), OsString::from("test")],
    ] {
        assert!(!is_calibration_candidate(&arguments));
    }
}

#[test]
fn test_adaptive_context_signature_consistency() {
    let config = default_config();
    let sig1 = adaptive_context_signature(&config);
    let sig2 = adaptive_context_signature(&config);
    assert_eq!(sig1, sig2);
    assert_eq!(sig1.len(), 16);
}

#[test]
fn test_calibration_signature_sensitivity() {
    let args1 = vec![OsString::from("git"), OsString::from("status")];
    let args2 = vec![OsString::from("git"), OsString::from("log")];
    let sig1 = calibration_signature(&args1, Some("C:\\repo"));
    let sig2 = calibration_signature(&args2, Some("C:\\repo"));
    let sig3 = calibration_signature(&args1, Some("C:\\other"));
    assert_ne!(sig1, sig2);
    assert_ne!(sig1, sig3);
}

#[test]
fn test_route_policy_validation() {
    let config = default_config();
    let context = adaptive_context_signature(&config);
    let policy = RoutePolicyFile {
        schema_version: ROUTE_POLICY_SCHEMA_VERSION,
        manifest_version: adapter_contract_id(),
        context_signature: context.clone(),
        evidence: vec![RoutePolicyEvidence {
            key: "git:status".to_owned(),
            raw_median_ms: 10.0,
            candidate_median_ms: 20.0,
            token_savings_percent: 0.0,
            sample_count: 5,
        }],
    };
    assert_eq!(
        policy.route_for("git:status", &context, PolicyObjective::Balanced),
        Some(Route::Raw)
    );
    assert_eq!(
        policy.route_for("git:status", "invalid_context", PolicyObjective::Balanced),
        None
    );
}
