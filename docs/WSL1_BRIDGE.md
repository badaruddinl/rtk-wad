# Isolated WSL1 Runtime and Native Bridge

## Purpose

The WSL1 profile provides a lower-overhead Linux execution candidate for RTK
without converting, unregistering, or otherwise modifying an existing WSL2
distribution. It is opt-in and selected through the canonical `xuva`
command.

The profile has two parts:

1. `Ubuntu-RTK-WSL1`, a dedicated Ubuntu 22.04 WSL1 distribution containing a
   non-root `rtk` user and RTK under `$HOME/.local/bin/rtk`.
2. `xuva --route wsl1`, an explicit route through the same canonical
   executable.

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
- writes a root-owned, read-only `/etc/xuva-dedicated-wsl1` marker with a
  unique installation ID;
- verifies `flock`, `setsid`, `tar`, `env`, and `sh`;
- writes the isolated distro's `/etc/wsl.conf`;
- restarts only the new distro and runs an RTK smoke test.

The downloaded images remain under `E:\luthfi\wsl\images`, but the default runtime
location is `%LOCALAPPDATA%\xuva\Ubuntu-RTK-WSL1` on NTFS. WSL1 expands its
root filesystem into ordinary host files and must not be installed on exFAT.
Existing WSL2 distros are never converted or terminated.

The runtime location and project location are separate concerns. Projects may
remain on NTFS, exFAT, ReFS, or another Windows filesystem that WSL exposes
through DrvFS. The profile is validated against the Flowpeek checkout on the
exFAT `E:` volume. Only the WSL1 Linux root filesystem requires the host
semantics provided by NTFS on this system.

The source checkout may remain on exFAT, but Rust build artifacts should use an
NTFS `CARGO_TARGET_DIR`. Cargo incremental hard-link behavior is not reliable on
the validated exFAT volume and can leave a stale executable timestamp.

## Installation and selection

Build and install the one supported command:

```powershell
cargo build --release
.\scripts\install.ps1
```

The installer retains atomic staging, refusal, backup, rollback, and recovery
behavior for `xuva.exe` only.

## Selection Contract

| XUVA route or configuration | Backend | Default distro |
|---|---|---|
| `xuva` | `auto` | `Ubuntu` |
| `xuva --route wsl1` | `wsl1` | `Ubuntu-RTK-WSL1` |
| `XUVA_WSL_BACKEND=wsl1` | `wsl1` | `Ubuntu-RTK-WSL1` |
| `XUVA_WSL_BACKEND=wsl2` | `wsl2` | `Ubuntu` |

`XUVA_WSL_DISTRO` overrides the default distro. `XUVA_WSL_BACKEND` explicitly
selects the WSL provider for the canonical command.

The Git router remains orthogonal to the WSL backend. Git launched from a native
Windows worktree still uses `git.exe` under `XUVA_WSL_GIT_MODE=auto`.

## Diagnostics

Use the explicit diagnostic path before dogfooding or after distro maintenance:

```powershell
xuva --route wsl1 --explain-route git --version
```

Expected output includes:

```text
route=wsl1
command_family=git
```

The explicit route reports a failure when the configured provider is unavailable.
Normal auto-routed commands avoid a full WSL-distribution scan on every call.

## Verification Gate

The WSL1 profile passed the following promotion gates:

- `cargo fmt --all --check`;
- `cargo clippy --all-targets -- -D warnings`;
- unit and packaging/recovery tests;
- the opt-in WSL1 process contract using
  `XUVA_WSL1_TEST_DISTRO=Ubuntu-RTK-WSL1`;
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

The WSL1 child runs in a separate Windows process group. Before the target can
run, it attests the dedicated installation ID and waits for a matching
parent-issued launch permit. On Ctrl+C or Ctrl+Break, including immediately
after `wsl.exe` is spawned, the bridge withholds that permit, kills the Windows
proxy, revalidates the expected installation ID, terminates only the dedicated
`Ubuntu-RTK-WSL1` distro, proves it stopped, and then releases the mutex. This
resets the WSL1 transport before the queued command starts. The distro must
remain dedicated to RTK because cancellation intentionally ends every process
inside that isolated runtime.

XUVA proves that the selected distro is version 1 and validates the root-owned
dedicated-runtime marker before execution and again before termination. A
renamed or overridden general-purpose distro without this marker is rejected
and is never terminated.

The Windows mutex makes Linux `flock` redundant for WSL1, and dedicated-distro
termination makes a Linux process group redundant for cancellation. The
optimized WSL1 launch therefore uses a small shell only to resolve the selected
user's home and establish a clean environment, then directly executes RTK. This
preserves portable user and path overrides without the previous `setsid` and
`flock` startup cost.

WSL2 retains Linux process-group signal escalation and is never terminated by
this profile-specific lifecycle. It now uses the same attest-then-permit
principle around a private cancellation token, preventing an early Ctrl+C from
racing ahead of token creation.

## Known Boundary

The bridge preserves structured argv into RTK. It cannot make RTK's own `run`
subcommand lossless because `run` intentionally reconstructs a shell command.
Use a structured RTK subcommand, `proxy`, or the target executable directly for
complex argv.
