# RTK-WAD

RTK Windows Adaptive Dispatcher. `rtk-wad` is a native Windows launcher that
chooses one safe execution route for each RTK command: raw native execution for
mutations, stock Windows RTK for verified structured adapters, or isolated
WSL1 RTK when Linux semantics are required. It forwards arguments as structured
process arguments and never rebuilds a shell command string.

In `auto` mode, route choice is an auditable local decision policy rather than
a fixed preference for RTK or WSL. For verified read-only command forms, WAD
uses repeated benchmark evidence to weigh end-to-end latency against token
reduction: it keeps raw execution when RTK has no meaningful saving and raw is
as fast or faster; it promotes native RTK when the measured candidate is faster
or when verified token saving reaches the 25% threshold. This token-first
threshold intentionally permits a small latency cost for materially smaller
agent context. Imported benchmark policy takes priority. Otherwise, WAD can
collect bounded local evidence across natural invocations of a safe read-only
command; mutations never become adaptive and no command is replayed to train
the policy.

```powershell
rtk-wad --explain-route rg -n pattern src
rtk-wad policy show
rtk-wad calibration show
rtk-wad gain
```

Current baseline: `0.1.0-alpha.1` (local stabilization). Project home:
`https://github.com/badaruddinl/rtk-wsl`.

## Build and use

```powershell
cargo build --release
.\target\release\rtk-wsl.exe rg "pattern" .
.\target\release\rtk-wad.exe rg "pattern" .
.\target\release\rtk-wad.exe gain
```

Install the release binary for the current Windows user:

```powershell
.\scripts\install.ps1
```

It installs `rtk-wad.exe`. The compatibility commands `rtk-wsl.exe` and
`rtk-wsl1.exe` remain opt-in aliases and may be installed with
`-CommandName rtk-wsl` or `-CommandName rtk-wsl1`. The installer refuses to
replace an existing executable unless `-Force` is supplied.

See [the adaptive routing contract](docs/RTK_WAD.md) for route selection,
structured-argument safety, local `gain` accounting, and non-NTFS source-volume
support.

The first real-corpus comparison is available in
[the Flowpeek three-way benchmark](docs/BENCHMARK_FLOWPEEK_2026-07-24.md). It
reports both token wins and the dispatcher latency cost; do not infer a speed
win from token savings alone.

The Windows Cargo route is separately validated on a non-NTFS worktree in
[the toolchain validation record](docs/TOOLCHAIN_VALIDATION_2026-07-24.md).
Its three-way decision evidence is recorded in
[the Cargo check benchmark](docs/BENCHMARK_CARGO_CHECK_2026-07-24.md).

The first Node package-manager comparison is recorded in
[the NPM run-list benchmark](docs/BENCHMARK_NPM_RUN_LIST_2026-07-24.md). It
selects raw Windows NPM because the tested read-only operation has no token
saving and lower end-to-end latency.

[The Go, Dart, and Flutter adapter benchmark](docs/BENCHMARK_GO_DART_FLUTTER_2026-07-24.md)
records real-project evidence for the Windows SDK shims. Its exact `go test
./...` policy is token-first; Dart and Flutter retain raw Windows execution
because they are WAD-owned shims with no stock RTK equivalent.

The first [on-demand provider discovery](docs/PROVIDER_DISCOVERY_PD1.md) slice
can inspect existing Go installations on Windows and WSL without installing or
changing any toolchain. Automatic cross-host selection remains intentionally
deferred until its path-mapping contract is proven.

[PD2 provider resolution](docs/PROVIDER_RESOLUTION_PD2.md) proves a Windows
project's actual mapping inside a candidate WSL distribution with structured
arguments before that provider can be reported as usable.

[PD3 provider-aware execution](docs/PROVIDER_EXECUTION_PD3.md) can use a
verified existing WSL Go+RTK provider when Windows Go is unavailable. It exits
cleanly with diagnostics when no safe provider exists; it never installs one.

[PD4 assisted setup planning](docs/ASSISTED_SETUP_PD4.md) exposes a reviewable
local `rtk-wad setup go` plan. It proposes at most one safe Windows Go command
and remains unable to apply it in this milestone.

[PD5 opt-in setup](docs/OPT_IN_SETUP_PD5.md) adds the separately confirmed
`setup go --apply --confirm` transaction, a local journal, and non-replaying
recovery. No installer is reached by normal routing, discovery, or planning.

[PD6 operational freeze](docs/SETUP_OPERATIONAL_FREEZE_PD6.md) supplies a
repeatable local readiness gate for this contract without invoking `winget`.

[P7 cache optimization and re-benchmark](docs/ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md)
documents the lazy WSL provider probe and the evidence-based automatic route
policy for the current head.

[P10 local adaptive calibration](docs/LOCAL_ADAPTIVE_CALIBRATION_P10.md)
documents the bounded candidate, provisional, and stable selection cycle used
when no imported benchmark policy exists.

[P11 provider baseline](docs/PROVIDER_BASELINE_P11.md) supplies the repeatable,
local-first inventory of Windows and WSL tool/RTK providers that gates the
generic cross-host registry work.

[P12 generic provider registry](docs/GENERIC_PROVIDER_REGISTRY_P12.md)
extends on-demand provider discovery beyond Go while retaining the P13 gate for
automatic cross-host execution.

[P13 bidirectional provider mapping](docs/BIDIRECTIONAL_PROVIDER_MAPPING_P13.md)
requires structured path conversion and a target-host directory probe in both
directions before a cross-host provider can be reported as usable. It remains a
diagnostic gate until P14 proves generic execution.

[P14 generic provider execution](docs/PROVIDER_EXECUTION_ENGINE_P14.md)
adds the explicit `provider exec` boundary for verified Windows and WSL
providers. Automatic command routing remains deferred until P15 classifies the
complete RTK command surface.

[P15 command-surface parity](docs/COMMAND_SURFACE_PARITY_P15.md) embeds the
full RTK `0.43.0` inventory and exposes `rtk-wad surface --json`; a process
contract compares all 69 command families with the live WSL RTK help output.

The external CLI adapters have a separate deterministic, network-free
[three-way fixture validation](docs/BENCHMARK_EXTERNAL_FIXTURES_2026-07-24.md).
It proves raw argv and native/WSL RTK equivalence without treating fixture
timings as live-service routing evidence.

The [filesystem matrix](docs/FILESYSTEM_MATRIX_2026-07-24.md) records the
native-route and local-ledger result on both the real exFAT source worktree and
a temporary NTFS worktree.

The [dogfood-cycle record](docs/DOGFOOD_CYCLES_2026-07-24.md) shows repeated
route decisions and cumulative token accounting on the two real local projects.

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

For an upgrade, rebuild first and use `-Force`; the previous executable is retained as `rtk-wad.exe.previous.exe`. To remove the adaptive launcher, run:

```powershell
.\scripts\uninstall.ps1
```

To restore the last backed-up executable instead, run `./scripts/uninstall.ps1 -RestorePrevious`. The retained `.cmd` fallback applies only when the optional `rtk-wsl` compatibility alias is installed and later removed.

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
- `RTK_WSL_EXTRA_PATH` (optional colon-separated absolute Linux directories prepended to the clean child `PATH`)
- `RTK_WAD_ROUTE` (`auto`, default; `raw`; `native-rtk`; `wsl1`; or `wsl2`)
- `RTK_WAD_NATIVE_RTK_PATH` (optional; defaults to `rtk.exe` on `PATH`)

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
reliably create a concurrent signal-helper session. Because the Windows mutex
already serializes WSL1 and cancellation resets the dedicated distro, the WSL1
launch path skips the redundant Linux `setsid` and `flock` layers. WSL2 retains
the Linux process-group and lock contract.

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
