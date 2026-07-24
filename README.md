# rtk-wsl

Native Windows launcher for the Linux RTK binary in WSL. It uses `wsl.exe --exec` and forwards every argument as structured process arguments; it does not rebuild a shell command string. Git commands started from a Windows-drive worktree use native `git.exe` by default, avoiding WSL `/mnt/<drive>` traversal and CRLF-index mismatches; every other RTK command remains in WSL.

Current stable release: `0.1.0`. Project home: `https://github.com/badaruddinl/rtk-wsl`.

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

An isolated WSL1 runtime is available as an opt-in development profile. It does
not convert or modify an existing WSL2 distribution. Enable the Windows WSL1
component from an elevated PowerShell, restart if requested, provision the
dedicated distribution, then install the executable alias:

```powershell
.\scripts\enable-wsl1.ps1
.\scripts\provision-wsl1.ps1
cargo build --release
.\scripts\install.ps1 -CommandName rtk-wsl1
rtk-wsl1 --bridge-info
```

`rtk-wsl1.exe` is the same Rust binary under an explicit command name. The
executable name selects the `wsl1` backend and the isolated
`Ubuntu-RTK-WSL1` distribution without adding a discovery process to every
normal invocation. See `docs/WSL1_BRIDGE.md` for the lifecycle and validation
contract.

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
- `RTK_WSL_BACKEND` (`auto`, default; `wsl1`; or `wsl2`)
- `RTK_WSL_USER` (optional; selects a specific WSL user)
- `RTK_WSL_RTK_PATH` (optional; defaults to `$HOME/.local/bin/rtk` inside WSL)
- `RTK_WSL_LOCK_PATH` (default: `/tmp/rtk-wsl.lock`)
- `RTK_WSL_LOCK_WAIT_SECONDS` (default: `120`)
- `RTK_WSL_CWD` (optional; an absolute Linux path for UNC shares or custom WSL mounts)
- `RTK_WSL_GIT_MODE` (`auto`, default; `native`; or `wsl`)

The `rtk-wsl1.exe` alias defaults to `RTK_WSL_BACKEND=wsl1` and
`RTK_WSL_DISTRO=Ubuntu-RTK-WSL1`. Explicit environment values override the
alias defaults. The normal `rtk-wsl.exe` command retains its existing `auto`
backend and `Ubuntu` distro defaults.

Every configured Linux path must be absolute. Empty values and a non-positive lock
timeout are rejected before WSL starts. The default path is derived by the fixed
launcher script from the selected WSL user's existing `HOME`; it does not probe or
cache user information for each invocation.

`RTK_WSL_GIT_MODE=auto` selects `git.exe` only when the caller is in a normal
Windows-drive worktree and no WSL `-C`, `--git-dir`, or `--work-tree` path is
supplied. This preserves exact Git argv and the user's Windows Git configuration.
Use `wsl` for a Linux worktree or when WSL Git is intentionally required; use
`native` to force native Git from another supported Windows context.
Native Git keeps the ordinary Windows console cancellation behavior; WSL commands
retain a backend-specific cancellation and lock-release contract. WSL2 uses the
dedicated Linux process group and never terminates the distro. The WSL1 profile
uses a Windows named mutex and a separate Windows process group; cancellation
terminates only the dedicated `Ubuntu-RTK-WSL1` runtime because Store WSL1 cannot
reliably create a concurrent signal-helper session.

Run `rtk-wsl --bridge-info` or `rtk-wsl1 --bridge-info` to print the selected
backend, distribution, detected WSL version, and Git mode. Diagnostics fail when
an explicit WSL1/WSL2 backend does not match the registered distro version. The
normal execution path deliberately does not query `wsl.exe --list --verbose`, so
version discovery adds no per-command overhead.

## Alpha verification

Run the Rust process contract on Windows with WSL available:

```powershell
cargo test
```

It covers literal arguments (including Unicode), stdout/stderr, exit codes,
interactive stdin, and Ctrl+Break cancellation releasing the shared lock. The
WSL2 launcher forwards Windows cancellation to only the Linux process group it
started and never terminates the distro. The isolated WSL1 profile uses the
dedicated-distro lifecycle described above. Run the installer/recovery contract
after a release build:

```powershell
cargo build --release
.\tests\packaging-contract.ps1
```

The packaging contract uses a temporary destination only; it does not change the
active launcher installation.

The first milestone is intentionally small: executable launch, lossless argv forwarding, clean Linux RTK environment, and exit-code propagation. Windows-tool shims, an optional `rtkw.exe` alias, and upstream contribution work remain in the queued milestones.

## License and upstream contribution

This proof of concept uses the Apache License 2.0 to match upstream RTK. It is marked `publish = false` and is not presented as an official RTK package.

Upstream contributions target the `develop` branch, require focused tests and documentation, and currently use a CLA Assistant workflow. Do not submit the code upstream until the contributor confirms that they own the contribution or have any employer permission required by the upstream contribution terms.
