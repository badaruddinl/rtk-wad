use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};

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
        "/updatedInput/command",
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

fn hook_claude(native_rtk_path: &str) -> ExitCode {
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin().read_to_end(&mut input) {
        eprintln!("rtk-wad: could not read Claude hook input: {error}");
        return ExitCode::FAILURE;
    }
    let mut command = Command::new(native_rtk_path);
    command
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("rtk-wad: native RTK Claude hook could not start: {error}");
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
            eprintln!("rtk-wad: native RTK Claude hook did not complete: {error}");
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
            eprintln!("rtk-wad: native RTK Claude hook emitted invalid JSON: {error}");
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

pub(crate) fn command(arguments: &[OsString], native_rtk_path: &str) -> ExitCode {
    match (
        arguments.get(1).and_then(|argument| argument.to_str()),
        arguments.get(2).and_then(|argument| argument.to_str()),
        arguments.len(),
    ) {
        (Some("hook"), Some("claude"), 3) => hook_claude(native_rtk_path),
        (Some("integration"), Some("claude"), 3) => {
            println!("RTK-WAD Claude integration is intentionally opt-in.");
            println!("1. Configure the stock native RTK hook with: rtk init -g");
            println!(
                "2. In the resulting agent hook registration, replace only `rtk hook claude` with `rtk-wad agent hook claude`."
            );
            println!(
                "3. Keep all other hook entries unchanged, restart Claude Code, then run a safe command such as git status."
            );
            println!(
                "The adapter delegates rewrite decisions to native RTK and changes only a rewritten `rtk ...` command into `rtk-wad ...`."
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("rtk-wad: usage: agent hook claude | agent integration claude");
            ExitCode::FAILURE
        }
    }
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
}
