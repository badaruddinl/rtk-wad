use crate::PRODUCT_COMMAND;
use crate::config::Config;

pub(crate) fn print_adapter_info(config: &Config) {
    println!("adapter={PRODUCT_COMMAND}");
    println!("command={PRODUCT_COMMAND}");
    println!("profile={}", config.profile.as_str());
    println!("route_preference={}", config.route_preference.as_str());
    println!("environment={}", config.environment.as_str());
    println!("policy_objective={}", config.policy_objective.as_str());
    println!(
        "environment_allowlist={}",
        if config.environment_allowlist.is_empty() {
            "none".to_owned()
        } else {
            config.environment_allowlist.join(",")
        }
    );
    println!("environment_boolean_feature_gates=automatic");
    println!("native_rtk_path={}", config.native_rtk_path);
    println!(
        "metrics={}",
        if config.metrics_enabled {
            "local-aggregate-only"
        } else {
            "off"
        }
    );
    println!(
        "calibration={}",
        if config.calibration_enabled {
            "local-opaque-bounded"
        } else {
            "off"
        }
    );
}
