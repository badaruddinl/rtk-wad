use std::collections::HashSet;
use std::env;
use std::ffi::OsString;

use crate::config::{Config, is_sensitive_environment_name};

pub(crate) fn forwarded_environment(config: &Config) -> Vec<(OsString, OsString)> {
    const SAFE_DEFAULTS: &[&str] = &[
        "CI",
        "COLORTERM",
        "FORCE_COLOR",
        "NO_COLOR",
        "RUST_BACKTRACE",
        "TERM",
    ];
    let explicitly_allowed: HashSet<&str> = config
        .environment_allowlist
        .iter()
        .map(String::as_str)
        .collect();
    env::vars_os()
        .filter(|(name, value)| {
            should_forward_environment(
                name.to_str().unwrap_or_default(),
                value.to_str(),
                &explicitly_allowed,
                SAFE_DEFAULTS,
            )
        })
        .collect()
}

pub(crate) fn should_forward_environment(
    name: &str,
    value: Option<&str>,
    explicitly_allowed: &HashSet<&str>,
    safe_defaults: &[&str],
) -> bool {
    if is_sensitive_environment_name(name) {
        return false;
    }
    let automatic_feature_gate = matches!(value, Some("0" | "1")) && name.contains("_RUN_");
    safe_defaults.contains(&name) || explicitly_allowed.contains(name) || automatic_feature_gate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn environment_forwarding_is_allowlisted_and_secret_averse() {
        let explicit = HashSet::from(["PROJECT_MODE", "GIT_AUTHOR_NAME"]);
        let defaults = ["CI"];
        assert!(should_forward_environment(
            "XPDE_RUN_TRAINING_E2E",
            Some("1"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "PROJECT_MODE",
            Some("training"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "CI",
            Some("true"),
            &explicit,
            &defaults
        ));
        assert!(should_forward_environment(
            "GIT_AUTHOR_NAME",
            Some("XUVA Contract"),
            &explicit,
            &defaults
        ));
        assert!(!should_forward_environment(
            "PROJECT_RUN_MODE",
            Some("training"),
            &explicit,
            &defaults
        ));
        assert!(!should_forward_environment(
            "PROJECT_SECRET_TOKEN",
            Some("1"),
            &HashSet::from(["PROJECT_SECRET_TOKEN"]),
            &defaults
        ));
        assert!(
            Config::from_lookup(
                |name| (name == "XUVA_ENV_ALLOWLIST").then(|| "SAFE_FLAG,API_TOKEN".to_owned())
            )
            .is_err()
        );
    }
}
