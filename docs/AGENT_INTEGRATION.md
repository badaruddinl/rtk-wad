# Agent integration

RTK-WAD is a dispatcher, not a replacement hook implementation. Its first
supported adapter is Claude Code and deliberately composes with the stock native
RTK hook instead of copying upstream's agent configuration logic.

## Claude Code

1. Install and initialize stock native RTK with `rtk init -g`.
2. Review the hook registration created by RTK. Replace only the exact command
   `rtk hook claude` with `rtk-wad agent hook claude`.
3. Leave every other agent hook entry unchanged, restart Claude Code, and test
   a read-only command such as `git status`.

`rtk-wad agent hook claude` forwards stdin to `rtk hook claude` through a
structured child process. On a successful JSON rewrite response, it changes
only `updatedInput.command` when the value begins with `rtk ` or `rtk.exe `;
the replacement is `rtk-wad `. It never parses the original shell command,
reconstructs argv, falls back to WSL, or replays a tool invocation.

The adapter fails clearly when native RTK is missing or returns invalid hook
JSON. It intentionally does not patch agent settings files itself: hook
registries can contain third-party handlers, and a blanket rewrite would be an
unsafe ownership violation. Use `rtk-wad agent integration claude` to print the
same setup instructions without invoking a hook.

Other agents continue to use their upstream RTK integrations until each has a
separate protocol and process-contract proof.
