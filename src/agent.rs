use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};

const SUPPORTED_HOOKS: &[&str] = &["claude", "cursor", "gemini", "copilot"];

fn supported_hook(name: &str) -> bool {
    SUPPORTED_HOOKS.contains(&name)
}

fn rewrite_hook_command(command: &mut String) -> bool {
    if let Some(rewritten) = command.strip_prefix("rtk ") {
        *command = format!("rtk-wad {rewritten}");
        true
    } else if let Some(rewritten) = command.strip_prefix("rtk.exe ") {
        *command = format!("rtk-wad {rewritten}");
        true
    } else {
        false
    }
}

fn rewrite_hook_payload(payload: &mut serde_json::Value) -> bool {
    for pointer in [
        "/hookSpecificOutput/updatedInput/command",
        "/hookSpecificOutput/tool_input/command",
        "/modifiedArgs/command",
        "/updatedInput/command",
        "/updated_input/command",
    ] {
        let Some(value) = payload.pointer_mut(pointer) else {
            continue;
        };
        let Some(mut command) = value.as_str().map(str::to_owned) else {
            continue;
        };
        if rewrite_hook_command(&mut command) {
            *value = serde_json::Value::String(command);
            return true;
        }
    }
    false
}

fn hook(agent: &str, native_rtk_path: &str) -> ExitCode {
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("rtk-wad: could not read {agent} hook input: {error}");
        return ExitCode::FAILURE;
    }
    let mut command = Command::new(native_rtk_path);
    command
        .args(["hook", agent])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("rtk-wad: native RTK {agent} hook could not start: {error}");
            return ExitCode::from(127);
        }
    };
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(&input)
    {
        eprintln!("rtk-wad: could not forward Claude hook input: {error}");
        return ExitCode::FAILURE;
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("rtk-wad: native RTK {agent} hook did not complete: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !output.status.success() {
        let _ = std::io::stdout().write_all(&output.stdout);
        let _ = std::io::stderr().write_all(&output.stderr);
        return ExitCode::from(output.status.code().unwrap_or(1) as u8);
    }
    let mut payload: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("rtk-wad: native RTK {agent} hook emitted invalid JSON: {error}");
            return ExitCode::FAILURE;
        }
    };
    rewrite_hook_payload(&mut payload);
    match serde_json::to_writer(std::io::stdout(), &payload) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rtk-wad: could not emit Claude hook JSON: {error}");
            ExitCode::FAILURE
        }
    }
}

fn initialization_command(agent: &str) -> &'static str {
    match agent {
        "claude" => "rtk init -g",
        "cursor" => "rtk init -g --agent cursor",
        "gemini" => "rtk init -g --gemini",
        "copilot" => "rtk init --copilot",
        _ => unreachable!("unsupported hook is rejected before rendering instructions"),
    }
}

fn print_integration(agent: &str) {
    println!("RTK-WAD {agent} integration is intentionally opt-in.");
    println!(
        "1. Configure the stock native RTK hook with: {}",
        initialization_command(agent)
    );
    println!(
        "2. In the resulting agent hook registration, replace only `rtk hook {agent}` with `rtk-wad agent hook {agent}`."
    );
    println!(
        "3. Keep all other hook entries unchanged, restart the agent, then run a safe command such as git status."
    );
    println!(
        "The adapter delegates rewrite decisions to native RTK and changes only a rewritten `rtk ...` command into `rtk-wad ...`."
    );
}

pub(crate) fn command(arguments: &[OsString], native_rtk_path: &str) -> ExitCode {
    let action = arguments.get(1).and_then(|argument| argument.to_str());
    let agent = arguments.get(2).and_then(|argument| argument.to_str());
    if arguments.len() == 3
        && let (Some(action), Some(agent)) = (action, agent)
    {
        if !supported_hook(agent) {
            eprintln!(
                "rtk-wad: unsupported agent `{agent}`; supported hooks: {}",
                SUPPORTED_HOOKS.join(", ")
            );
            return ExitCode::FAILURE;
        }
        return match action {
            "hook" => hook(agent, native_rtk_path),
            "integration" => {
                print_integration(agent);
                ExitCode::SUCCESS
            }
            _ => {
                eprintln!(
                    "rtk-wad: usage: agent hook <agent> | agent integration <agent> (agents: {})",
                    SUPPORTED_HOOKS.join(", ")
                );
                ExitCode::FAILURE
            }
        };
    }
    eprintln!(
        "rtk-wad: usage: agent hook <agent> | agent integration <agent> (agents: {})",
        SUPPORTED_HOOKS.join(", ")
    );
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_upstream_rtk_updated_input() {
        let mut claude = serde_json::json!({
            "hookSpecificOutput": {
                "updatedInput": { "command": "rtk git status --short" },
                "permissionDecision": "allow"
            }
        });
        assert!(rewrite_hook_payload(&mut claude));
        assert_eq!(
            claude.pointer("/hookSpecificOutput/updatedInput/command"),
            Some(&serde_json::json!("rtk-wad git status --short"))
        );
        assert_eq!(
            claude.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::json!("allow"))
        );

        let mut unrelated = serde_json::json!({ "updatedInput": { "command": "git status" } });
        assert!(!rewrite_hook_payload(&mut unrelated));
    }

    #[test]
    fn rewrites_each_supported_native_hook_output_shape() {
        for mut payload in [
            serde_json::json!({ "updated_input": { "command": "rtk git status" } }),
            serde_json::json!({ "hookSpecificOutput": { "tool_input": { "command": "rtk git status" } } }),
            serde_json::json!({ "modifiedArgs": { "command": "rtk.exe git status" } }),
        ] {
            assert!(rewrite_hook_payload(&mut payload));
            let rendered = payload.to_string();
            assert!(rendered.contains("rtk-wad git status"));
            assert!(!rendered.contains("rtk.exe git status"));
        }
    }

    #[test]
    fn only_advertises_supported_hook_protocols() {
        assert!(supported_hook("claude"));
        assert!(supported_hook("cursor"));
        assert!(supported_hook("gemini"));
        assert!(supported_hook("copilot"));
        assert!(!supported_hook("codex"));
        assert_eq!(initialization_command("gemini"), "rtk init -g --gemini");
    }
}
