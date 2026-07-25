# Agent integration

RTK-WAD is a dispatcher, not a replacement hook implementation. Its adapters
compose with stock native RTK hooks instead of copying upstream's rewrite logic
or modifying agent configuration files.

## Supported hook protocols

| Agent | WAD hook command | Stock RTK initialization command |
| --- | --- | --- |
| Claude Code | `rtk-wad agent hook claude` | `rtk init -g` |
| Cursor | `rtk-wad agent hook cursor` | `rtk init -g --agent cursor` |
| Gemini CLI | `rtk-wad agent hook gemini` | `rtk init -g --gemini` |
| GitHub Copilot | `rtk-wad agent hook copilot` | `rtk init --copilot` |

For a supported agent:

1. Run its stock RTK initialization command from the table.
2. Review the generated hook registration. Replace only the exact command
   `rtk hook <agent>` with `rtk-wad agent hook <agent>`.
3. Leave every other hook entry unchanged, restart the agent, and test a
   read-only command such as `git status`.

`rtk-wad agent hook <agent>` forwards stdin to `rtk hook <agent>` through a
structured child process. On a successful JSON rewrite response, it changes
only the command field published by the agent protocol when it begins with
`rtk ` or `rtk.exe `; the replacement is `rtk-wad `. Supported output shapes
are Claude/Copilot `updatedInput`, Cursor `updated_input`, Gemini
`hookSpecificOutput.tool_input`, and Copilot CLI `modifiedArgs`. It never
parses the original shell command, reconstructs argv, falls back to WSL, or
replays a tool invocation.

The adapter fails clearly when native RTK is missing or returns invalid hook
JSON. It intentionally does not patch agent settings files itself: hook
registries can contain third-party handlers, and a blanket rewrite would be an
unsafe ownership violation. Use `rtk-wad agent integration <agent>` to print
the matching setup instructions without invoking a hook.

Other agents continue to use their upstream RTK integrations until they have a
separate protocol and process-contract proof. Prompt-only integrations, such as
Codex CLI rules, do not expose a machine-readable hook boundary that WAD can
adapt safely.

## Verification

After the registration change, an isolated native-hook probe can verify the
handoff without running a project command:

```powershell
'{"tool_name":"Bash","tool_input":{"command":"git status"}}' |
  rtk-wad agent hook claude
```

With stock RTK v0.43.0, the successful response contains
`updatedInput.command` with `rtk-wad git status`. The same probe has been
checked against all four native protocols: Claude and VS Code Copilot emit
`updatedInput`, Gemini emits `hookSpecificOutput.tool_input`, and Copilot CLI
emits `modifiedArgs`. Cursor correctly passes through (`{}`) when native RTK's
permission policy declines an automatic rewrite; the adapter preserves that
decision unchanged. The Cursor `updated_input` rewrite shape is additionally
covered by the Rust contract test.

A missing native RTK is an explicit failure; the adapter never substitutes WSL
for an agent hook.
