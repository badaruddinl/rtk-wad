# XUVA migration

`xuva` is the canonical command and executable name beginning with the 0.4
line. It remains the same adaptive Windows dispatcher: it selects a verified
raw Windows, native RTK, WSL1, or WSL2 execution plan without treating a
missing provider as a route to force.

## Compatibility boundary

The installer places both launchers in its destination:

- `xuva.exe` is the supported command for new usage.
- `rtk-wad.exe` is a binary compatibility shim that executes the same program.

Consequently, existing shell aliases, scheduled tasks, agent hook
registrations, and scripts can migrate one invocation at a time. Both commands
keep process arguments, CWD, stdin/stdout/stderr, cancellation, and exit-code
semantics identical.

The following legacy compatibility surfaces are intentionally retained during
the migration window:

- `RTK_WAD_*` configuration and state environment variables.
- `%LOCALAPPDATA%\rtk-wad` cache and metrics state.
- `scripts/rtk-wad-wsl.sh` and `RTK_WAD_WINDOWS_EXE` for WSL-origin calls.

New WSL integrations should use `scripts/xuva-wsl.sh` and
`XUVA_WINDOWS_EXE`; the new shim falls back to the legacy variable when needed.

## Release and repository identity

The existing GitHub repository, historic tags, release archives, and v0.3.0
documentation remain under the RTK-WAD name. Changing a remote repository,
release URLs, or a public package identifier is an external cutover and is not
performed by this compatibility migration. Release archives from the XUVA line
contain both `xuva.exe` and `rtk-wad.exe`.
