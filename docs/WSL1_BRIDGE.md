# Isolated WSL1 Runtime and Native Bridge

## Purpose

The WSL1 profile provides a lower-overhead Linux execution candidate for RTK
without converting, unregistering, or otherwise modifying an existing WSL2
distribution. It is opt-in and does not change the stable `rtk-wsl` defaults.

The profile has two parts:

1. `Ubuntu-RTK-WSL1`, a dedicated Ubuntu 22.04 WSL1 distribution containing a
   non-root `rtk` user and RTK under `$HOME/.local/bin/rtk`.
2. `rtk-wsl1.exe`, an alias of the same Rust bridge binary. Its executable name
   selects the `wsl1` backend and the isolated distro without runtime discovery.

## Prerequisites

WSL1 requires the legacy `Microsoft-Windows-Subsystem-Linux` optional Windows
component in addition to the Microsoft Store WSL package. Enabling it is a
machine-level operation that requires an elevated PowerShell and may require a
Windows restart:

```powershell
.\scripts\enable-wsl1.ps1
```

The script uses the documented `wsl.exe --install --enable-wsl1
--no-distribution` path. It does not install or modify a distro.

## Provisioning

Run from a non-elevated PowerShell after the prerequisite and any required
restart:

```powershell
.\scripts\provision-wsl1.ps1
```

The provisioner:

- downloads the Canonical Ubuntu 22.04 WSL rootfs;
- verifies it against Canonical's published SHA-256 list;
- imports only `Ubuntu-RTK-WSL1` with `--version 1`;
- downloads the selected official RTK Linux musl archive;
- verifies it against the RTK release checksum list;
- downloads the selected official ripgrep Linux musl archive;
- verifies it against the ripgrep release checksum;
- creates the non-root `rtk` user;
- installs RTK and ripgrep under `/home/rtk/.local/bin`;
- verifies `flock`, `setsid`, `tar`, `env`, and `sh`;
- writes the isolated distro's `/etc/wsl.conf`;
- restarts only the new distro and runs an RTK smoke test.

The downloaded images remain under `E:\luthfi\wsl\images`, but the default runtime
location is `%LOCALAPPDATA%\rtk-wsl\Ubuntu-RTK-WSL1` on NTFS. WSL1 expands its
root filesystem into ordinary host files and must not be installed on exFAT.
Existing WSL2 distros are never converted or terminated.

The source checkout may remain on exFAT, but Rust build artifacts should use an
NTFS `CARGO_TARGET_DIR`. Cargo incremental hard-link behavior is not reliable on
the validated exFAT volume and can leave a stale executable timestamp.

## Bridge Installation

Build once and install the normal command or either alias independently:

```powershell
cargo build --release
.\scripts\install.ps1
.\scripts\install.ps1 -CommandName rtk-wsl1
```

The installer retains the same atomic staging, refusal, backup, rollback, and
recovery behavior for both command names. Remove only the WSL1 alias with:

```powershell
.\scripts\uninstall.ps1 -CommandName rtk-wsl1
```

## Selection Contract

| Command or configuration | Backend | Default distro |
|---|---|---|
| `rtk-wsl.exe` | `auto` | `Ubuntu` |
| `rtk-wsl1.exe` | `wsl1` | `Ubuntu-RTK-WSL1` |
| `RTK_WSL_BACKEND=wsl1` | `wsl1` | `Ubuntu-RTK-WSL1` |
| `RTK_WSL_BACKEND=wsl2` | `wsl2` | `Ubuntu` |

`RTK_WSL_DISTRO` overrides the default distro. `RTK_WSL_BACKEND` explicitly
overrides the executable-name profile.

The Git router remains orthogonal to the WSL backend. Git launched from a native
Windows worktree still uses `git.exe` under `RTK_WSL_GIT_MODE=auto`, including
through `rtk-wsl1.exe`.

## Diagnostics

Use the explicit diagnostic path before dogfooding or after distro maintenance:

```powershell
rtk-wsl1 --bridge-info
```

Expected output includes:

```text
bridge=rtk-wsl
backend=wsl1
distro=Ubuntu-RTK-WSL1
detected_wsl_version=1
git_mode=auto
```

Diagnostics return failure when the distro is missing or its registered WSL
version conflicts with an explicit backend. Normal commands skip this discovery
to avoid adding a `wsl.exe --list --verbose` call to every invocation.

## Verification Gate

The WSL1 profile passed the following promotion gates:

- `cargo fmt --all --check`;
- `cargo clippy --all-targets -- -D warnings`;
- unit and packaging/recovery tests;
- the opt-in WSL1 process contract using
  `RTK_WSL1_TEST_DISTRO=Ubuntu-RTK-WSL1`;
- literal argv, Unicode, stdout/stderr, exit 0/1/42/127, interactive stdin, and
  cancellation/lock release;
- warm and after-idle latency comparison against WSL2 and native Windows RTK;
- output-equivalence and token-reduction comparison.

The detailed results and benchmark values are recorded in
`docs/WSL1_VALIDATION.md`. WSL1 is a supported experimental bridge profile for
opt-in dogfooding. The stable WSL2/native-Git route remains the default.

## WSL1 Lock Compatibility

Store WSL1 on the validated host could not reliably create a second concurrent
`wsl.exe --exec` session and returned an internal `EventFd` error. Linux-only
locking therefore occurred too late. The WSL1 bridge acquires a Windows named
mutex before launching `wsl.exe`; waiting bridge processes never ask WSL1 to
create a competing session.

The WSL1 child runs in a separate Windows process group. On Ctrl+C or Ctrl+Break,
the bridge kills the child proxy, terminates only the dedicated
`Ubuntu-RTK-WSL1` distro, waits for exit, and then releases the mutex. This resets
the WSL1 transport before the queued command starts. The distro must remain
dedicated to RTK because cancellation intentionally ends every process inside
that isolated runtime.

WSL2 retains the original Linux process-group signal contract and is never
terminated by this profile-specific lifecycle.

## Known Boundary

The bridge preserves structured argv into RTK. It cannot make RTK's own `run`
subcommand lossless because `run` intentionally reconstructs a shell command.
Use a structured RTK subcommand, `proxy`, or the target executable directly for
complex argv.
