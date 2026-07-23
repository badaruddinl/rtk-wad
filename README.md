# rtk-wsl

Native Windows launcher for the Linux RTK binary in WSL. It uses `wsl.exe --exec` and forwards every argument as structured process arguments; it does not rebuild a shell command string.

Current milestone: `0.1.0-alpha.1`. Project home: `https://github.com/badaruddinl/rtk-wsl`.

## Build and use

```powershell
cargo build --release
.\target\release\rtk-wsl.exe rg "pattern" .
.\target\release\rtk-wsl.exe stats
```

Install the release binary for the current Windows user:

```powershell
.\scripts\install.ps1
```

It installs `rtk-wsl.exe` beside the existing `rtk-wsl.cmd`. Windows resolves the `.exe` first; removing that one file restores the previous wrapper. The installer refuses to replace an existing `.exe` unless `-Force` is supplied.

For an upgrade, rebuild first and use `-Force`; the previous executable is retained as `rtk-wsl.exe.previous.exe`. To remove the Rust launcher and fall back to the retained `.cmd` wrapper, run:

```powershell
.\scripts\uninstall.ps1
```

To restore the last backed-up executable instead, run `./scripts/uninstall.ps1 -RestorePrevious`.

The launcher runs RTK through `flock` and a clean Linux environment, preserving the existing tracking lock behavior. `stats` remains a compatibility alias for RTK `gain`.

## Configuration

By default, the launcher uses the selected distro's default user and that user's
`$HOME/.local/bin/rtk`. Override only when needed:

- `RTK_WSL_DISTRO` (default: `Ubuntu`)
- `RTK_WSL_USER` (optional; selects a specific WSL user)
- `RTK_WSL_RTK_PATH` (optional; defaults to `$HOME/.local/bin/rtk` inside WSL)
- `RTK_WSL_LOCK_PATH` (default: `/tmp/rtk-wsl.lock`)
- `RTK_WSL_LOCK_WAIT_SECONDS` (default: `120`)

The first milestone is intentionally small: executable launch, lossless argv forwarding, clean Linux RTK environment, and exit-code propagation. Windows-tool shims, an optional `rtkw.exe` alias, and upstream contribution work remain in the queued milestones.

## License and upstream contribution

This proof of concept uses the Apache License 2.0 to match upstream RTK. It is marked `publish = false` and is not presented as an official RTK package.

Upstream contributions target the `develop` branch, require focused tests and documentation, and currently use a CLA Assistant workflow. Do not submit the code upstream until the contributor confirms that they own the contribution or have any employer permission required by the upstream contribution terms.
