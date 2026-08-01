use crate::test_support::*;

#[test]
fn version_commands_are_owned_by_the_dispatcher() {
    for argument in [VERSION_ARGUMENT, "version", "-V"] {
        assert!(
            is_version_command(&[OsString::from(argument)]),
            "{argument}"
        );
    }
    assert!(is_verbose_version_command(&[
        OsString::from("--version"),
        OsString::from("--verbose")
    ]));
    assert!(!is_version_command(&[
        OsString::from("go"),
        OsString::from("version")
    ]));
}

#[test]
fn forwards_special_characters_as_distinct_arguments() {
    let arguments = rtk_arguments(
        vec![
            OsString::from("run"),
            OsString::from("semi;and&dollar$HOME"),
            OsString::from("C:\\Program Files\\Example"),
        ],
        &default_config(),
        "0123456789abcdef0123456789abcdef",
    );

    assert!(arguments.contains(&OsString::from("--exec")));
    assert!(arguments.contains(&OsString::from(LAUNCH_SCRIPT)));
    assert!(arguments.contains(&OsString::from("semi;and&dollar$HOME")));
    assert!(arguments.contains(&OsString::from("C:\\Program Files\\Example")));
}

#[test]
fn wsl_bridge_payload_preserves_literal_utf8_argv_without_shell_parsing() {
    let fields = decode_wsl_bridge_fields("Z28AcnVuAHNwYWNlICYgJGRvbGxhclzmvKLlrZcA")
        .expect("valid base64 payload decodes");
    assert_eq!(
        fields,
        vec![
            "go".to_owned(),
            "run".to_owned(),
            "space & $dollar\\\u{6f22}\u{5b57}".to_owned(),
        ]
    );
    assert!(decode_wsl_bridge_fields("Z28=").is_err());
    assert!(decode_wsl_bridge_fields("not base64!").is_err());
}

#[test]
fn wsl_bridge_request_carries_context_and_arguments_without_environment() {
    let request = wsl_bridge_request(&[OsString::from(
            "--wsl-bridge=djMAVWJ1bnR1AGJhZGFyAC9tbnQvZC9maXh0dXJlAEQ6XGZpeHR1cmUAL3RtcC9maXh0dXJlAHJhdwAtLWV4cGxhaW4tcm91dGUAZ28AcnVuAHgA",
        )])
        .expect("bridge payload is valid")
        .expect("argument selects the bridge");
    assert_eq!(request.distro, "Ubuntu");
    assert_eq!(request.origin_user, "badar");
    assert_eq!(request.cwd, "/mnt/d/fixture");
    assert_eq!(request.windows_cwd.as_deref(), Some(r"D:\fixture"));
    assert_eq!(request.extra_path.as_deref(), Some("/tmp/fixture"));
    assert_eq!(request.output_adapter, OutputAdapterPreference::Raw);
    assert_eq!(
        request.arguments,
        vec![
            OsString::from("--explain-route"),
            OsString::from("go"),
            OsString::from("run"),
            OsString::from("x"),
        ]
    );
}

#[test]
fn shell_operator_and_update_check_ux_are_owned_by_xuva() {
    assert!(is_shell_operator_command(&[OsString::from("&&")]));
    assert!(!is_shell_operator_command(&[
        OsString::from("rg"),
        OsString::from("literal && value")
    ]));
    let tags = "a refs/tags/v0.3.0\nb refs/tags/not-semver\nc refs/tags/v0.4.1\n";
    assert_eq!(
        latest_release_from_ls_remote(tags).as_deref(),
        Some("v0.4.1")
    );
    assert_eq!(parsed_stable_version("v1.2.3"), Some((1, 2, 3)));
    assert_eq!(parsed_stable_version("v1.2.3-rc1"), None);
    assert!(stable_release_is_newer("v1.2.3", "1.2.3-beta.1"));
    assert!(stable_release_is_newer("v1.2.4", "1.2.3-beta.1"));
    assert!(!stable_release_is_newer("v1.2.2", "1.2.3-beta.1"));
    assert!(!stable_release_is_newer("v1.2.3", "1.2.3"));
}
