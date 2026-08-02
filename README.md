<p align="center">
  <img src="assets/xuva-routing-hero.png" alt="XUVA routing a command through verified local and configured environments" width="100%" />
</p>

<h1 align="center">XUVA</h1>

<p align="center">
  <strong>Adaptive command dispatcher.</strong><br />
  One safe command boundary with explainable route selection across local and configured environments.
</p>

<p align="center">
  <a href="https://github.com/badsleepyday/xuva/actions/workflows/windows-ci.yml"><img src="https://github.com/badsleepyday/xuva/actions/workflows/windows-ci.yml/badge.svg?branch=master" alt="Windows CI" /></a>
  <a href="https://github.com/badsleepyday/xuva/tags"><img src="https://img.shields.io/github/v/tag/badsleepyday/xuva?sort=semver&label=version" alt="Version tag" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="Apache 2.0 license" /></a>
  <a href="docs/RELEASE_GATE_P20.md"><img src="https://img.shields.io/badge/status-public_beta-orange.svg" alt="Public beta status" /></a>
</p>

XUVA is an adaptive command dispatcher. For each command, it selects one
auditable route from the available local and configured providers. It preserves
arguments as structured process arguments and keeps execution local when
another route does not provide verified value.

> **Hard cutover.** Starting with `v0.4.1`, the project, repository, binary,
> installer, environment variables, and state paths use `xuva`. There is no
> legacy launcher in this release line. GitHub redirects the former repository
> URL for historic tags and evidence.

## Why XUVA

```mermaid
flowchart LR
    A[Command and argv] --> B[XUVA]
    B --> C{Safe, verified local evidence?}
    C -->|Mutation or no benefit| D[Raw local execution]
    C -->|Adapter capability helps| E[Configured output adapter]
    C -->|Provider route is verified| F[Configured environment provider]
    D --> G[One exit code, stdout, and stderr contract]
    E --> G
    F --> G
```

| Route | When XUVA uses it | What it protects |
| --- | --- | --- |
| Raw local | Mutations, unknown commands, or no verified adapter benefit | Lowest avoidable latency and original tool behavior |
| Output adapter | A verified read-only command has a useful adapted result | Compact output without shell reconstruction |
| Environment provider | A verified provider and path mapping require another environment | Structured cross-host execution, not ad-hoc shell quoting |

The dispatcher never replays a command merely to train its policy. Mutating
commands do not become adaptive. Provider discovery is local-first and does not
install a language runtime or tool automatically.

### Optional integration

[RTK](https://github.com/rtk-ai/rtk) is an optional output-adapter dependency.
When it is installed and a verified route benefits from it, XUVA can select it;
otherwise XUVA continues with raw or other verified provider routes.

## Download the verified Windows binary

The current Windows x86_64 release is `v0.4.4`, using the XUVA product and
binary identity across launcher, installer, environment variables, and state paths.
The historic `v0.3.0` archive is retained under its original filename and built from
the immutable tag, published with a SHA-256 sidecar, and accompanied by a GitHub
build-provenance attestation. It is not Authenticode-signed; see [release
provenance](docs/RELEASE_PROVENANCE.md) for the explicit trust boundary.

Supported public-beta runtime targets are Windows 10/11 x86_64 with PowerShell
5.1 or newer. Cross-host routes support Ubuntu under WSL1 or WSL2 when that
distro and its project mapping are verified. Other distributions can be
discovered, but are not part of the release gate. Source builds require Rust
1.95.0 or newer; official binaries use the pinned Rust 1.97.1 toolchain.

- [Download the v0.4.4 Windows archive](https://github.com/badsleepyday/xuva/releases/download/v0.4.4/xuva-v0.4.4-windows-x86_64.zip)
- [Download its SHA-256 sidecar](https://github.com/badsleepyday/xuva/releases/download/v0.4.4/xuva-v0.4.4-windows-x86_64.zip.sha256)
- [Open the v0.4.4 release record](https://github.com/badsleepyday/xuva/releases/tag/v0.4.4)

Verify the archive after downloading it:

```powershell
Get-FileHash .\xuva-v0.4.4-windows-x86_64.zip -Algorithm SHA256
Get-Content .\xuva-v0.4.4-windows-x86_64.zip.sha256
```

Every stable release also requires successful hosted quality gates and a
recorded self-hosted [Windows/WSL process-contract run](docs/SELF_HOSTED_WSL_CI.md)
for the release commit.

## Quick start

Build a release binary on Windows:

```powershell
cargo build --release --locked --bins
.\target\release\xuva.exe --version
.\target\release\xuva.exe --explain-route rg -n "pattern" src
```

Run this self-build through Cargo directly (or from a different XUVA binary).
Windows cannot replace `target\release\xuva.exe` while that same executable is
still acting as the dispatcher for the build.

Install it for the current user:

```powershell
.\scripts\install.ps1 -AddToPath
xuva gain
xuva self-update --check
```

Use `xuva install --status` to inspect the installation, `xuva rollback` to
swap to the retained previous complete bundle after the current process exits,
and `xuva uninstall --remove-from-path` to remove both bundle generations and
their PATH entry.
The default installation is the dedicated managed directory
`%LOCALAPPDATA%\Programs\XUVA`, never the shared `%USERPROFILE%\.local\bin`.
Every managed generation has a validated `.xuva-installation.json` ownership
marker; upgrade, rollback, and uninstall refuse a directory containing foreign
or missing files. A legacy `.local\bin\xuva.exe` is reported but never moved or
deleted automatically.

The companion scripts expose the same operations for recovery when the binary
cannot start. After an interrupted filesystem transaction, run
`.\scripts\install.ps1 -Recover` (or the installed `install.ps1 -Recover`)
before retrying the operation. The installer performs `xuva scan` after
activating the binary. It
inventories every launchable executable on the Windows `PATH` and the WSL
distros that actually exist, without executing those tools, installing a
toolchain, or forcing WSL. The dispatcher resolves and caches any safe
executable name on demand; use `xuva scan nvm rustc` to refresh named
providers across Windows and WSL explicitly.

The installer smoke-tests the candidate binary before activation and restores
the previous complete bundle if copying, activation, PATH update, or the
post-install provider scan fails. Official archives are checksum- and
metadata-verified before any activation. `self-update`
is deliberately diagnostic-only: `--check` queries stable tags through Git for
Windows, while installation still requires a reviewed release or trusted
checkout.

The core installer has no Python or tokenizer dependency. The pinned
[`tiktoken`](requirements/xuva-tokenizer.txt) environment is optional and used
only to reproduce benchmark token counts. Install it explicitly when running a
benchmark; it never alters the global Python environment. On a fresh PC without
Python, inspect the plan first:

```powershell
.\scripts\install-tokenizer.ps1 -PlanPythonBootstrap
```

Only `-InstallTokenizer -InstallPython -ConfirmPythonInstall` can install the
documented Python dependency together with the optional tokenizer.

To prevent automatic WSL selection for a Windows-only workflow, use the
per-command environment mode. It keeps RTK meta commands on native RTK and
runs unverified external commands directly on Windows:

```powershell
xuva --environment windows-only --explain-route pytest -q
$env:XUVA_ENVIRONMENT = "windows-only"
```

## Agent hook adapters

XUVA provides conservative adapters for the native RTK hooks used by Claude
Code, Cursor, Gemini CLI, and GitHub Copilot. Each adapter delegates rewrite
decisions to stock RTK, then changes only an emitted
`rtk ...` command into `xuva ...`; it does not parse or rebuild agent shell
commands itself. The registration is deliberately opt-in so existing agent
hooks are not silently changed:

```powershell
xuva agent integration claude
xuva agent integration cursor
xuva agent integration gemini
xuva agent integration copilot
```

Follow the printed three-step setup, then use the matching `xuva agent hook
<agent>` command in the hook registration. See [agent
integration](docs/AGENT_INTEGRATION.md) for the supported boundary and failure
behavior.

## A route decision you can inspect

XUVA exposes the policy decision instead of hiding it. This is a captured
local example from the repository's current release binary; another machine or
command form may choose differently.

```text
> xuva --explain-route rg -n XUVA src
route=native-rtk
reason=local calibration candidate: first safe observation uses native RTK
command_family=rg
```

Use `xuva policy show` and `xuva calibration show` to inspect the local
evidence behind later decisions.

Adaptive calibration is local, opaque, bounded, and **on by default**. It is
independent from the opt-in aggregate metrics ledger: `XUVA_METRICS=off` does
not prevent safe eligible commands from learning, and calibration never creates
`metrics-v1.sqlite`. Set `XUVA_CALIBRATION=off` to disable both calibration
reads and writes.

Set `XUVA_POLICY_OBJECTIVE=latency`, `balanced`, or `tokens` to choose the
evidence objective. `balanced` is the default and retains the documented 25%
token-saving threshold; `latency` chooses the lower measured median, while
`tokens` prefers any measured positive token saving. Objective identity is part
of the local evidence context, so changing it cannot reuse an incompatible
calibration decision.

## Benchmark result: latest public-corpus audit matrix

The current matrix covers three pinned public corpora, small/medium/large raw
outputs, four variants, and ten rotating measured rounds. All 480 performance
samples completed with exit code zero. Each corpus also passed a separate
failure contract: direct Git and XUVA explicit-raw both returned exit `128` with
byte-identical stdout/stderr. Token counts use `tiktoken==0.12.0`. The artifact
binds the results to Windows, CPU, Node, corpus commits, and binary SHA-256.

| Corpus | Workload | Output | Raw Windows | Stock RTK | XUVA forced | XUVA auto | Raw → auto tokens |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |
| pytest 8.4.0 | Git status | small | 100.605 ms | 216.360 ms | 222.709 ms | 156.661 ms | 6 → 6 |
| pytest 8.4.0 | Git log | small | 97.964 ms | 152.996 ms | 181.956 ms | 139.392 ms | 14 → 14 |
| pytest 8.4.0 | focused `rg` | medium | 106.070 ms | 155.264 ms | 173.942 ms | 142.647 ms | 1,990 → 1,990 |
| pytest 8.4.0 | broad `rg` | large | 90.762 ms | 267.460 ms | 272.463 ms | 452.577 ms | 687,067 → 6,213 |
| TypeScript 5.9.3 (`src` sparse) | Git status | small | 168.893 ms | 367.009 ms | 464.592 ms | 144.526 ms | 6 → 6 |
| TypeScript 5.9.3 (`src` sparse) | Git log | small | 81.736 ms | 149.837 ms | 176.307 ms | 118.086 ms | 18 → 18 |
| TypeScript 5.9.3 (`src` sparse) | focused `rg` | medium | 102.098 ms | 180.521 ms | 186.345 ms | 160.543 ms | 3,864 → 3,864 |
| TypeScript 5.9.3 (`src` sparse) | broad `rg` | large | 158.202 ms | 1,097.599 ms | 1,185.928 ms | 1,156.948 ms | 4,225,359 → 4,683 |
| ripgrep 14.1.1 | Git status | small | 97.296 ms | 224.006 ms | 233.135 ms | 122.995 ms | 6 → 6 |
| ripgrep 14.1.1 | Git log | small | 67.354 ms | 116.654 ms | 128.940 ms | 114.529 ms | 11 → 11 |
| ripgrep 14.1.1 | focused `rg` | small | 60.626 ms | 119.743 ms | 143.737 ms | 86.864 ms | 119 → 119 |
| ripgrep 14.1.1 | broad `rg` | large | 51.792 ms | 165.708 ms | 166.046 ms | 68.864 ms | 137,700 → 137,700 |

The matrix shows both costs and benefits. Most raw-equivalent automatic rows
were slower on this host; TypeScript Git status was the sole latency win. The
two broad searches that selected RTK reduced output drastically but traded
latency for tokens. “First observation” means fresh isolated XUVA state only;
OS and external-tool caches were uncontrolled. These are evidence-bound samples,
not a universal speed claim or a causal before/after claim.

See the reproducible [benchmark protocol](benchmarks/README.md), the
[versioned v0.4.4 matrix summary](benchmarks/evidence/v044-audit-core-matrix-summary.json), the historical
[comparison and methodology](docs/BENCHMARK_COMPARISON_P20.md), the
[full Windows/WSL matrix](docs/BENCHMARK_CORE_MATRIX_P18_2026-07-25.md), and
[machine-readable historical evidence](benchmarks/evidence/p18-comparison-summary.json).
Older results remain intentionally available because token savings and latency
are corpus-, pattern-, provider-, and host-specific.

### Read `gain` honestly

`xuva gain` is local RTK tracker accounting, not a benchmark runner and not
a raw-token estimator. It reports all invocations, but only native/WSL RTK
routes contribute RTK-measured token fields. Raw-route invocations are retained
as explicitly **unmeasured**; XUVA does not invent a token estimate for them.

Token saving is also not a promise of lower API cost or lower latency. Prompt,
system, conversation, output, and model pricing all affect an eventual bill.

## Windows and WSL safety contract

- Exact argv forwarding handles spaces, quotes, Unicode, `&`, `;`, `$`, and
  backslashes without shell reconstruction.
- Native `.exe` arguments remain direct process argv. Windows `.cmd` and `.bat`
  providers accept literal percent, exclamation, caret, quote, empty, and
  trailing-backslash arguments under the Windows batch boundary, but reject CR
  or LF before process creation.
- Paths are translated only in command-defined path positions (`git -C`,
  Cargo manifest/target flags, Go path flags, response files, and Git
  pathspecs). Generic data that merely resembles a path remains unchanged.
- Git operations in Windows worktrees use Git for Windows for NTFS object
  writes, credentials, and Windows DNS. Mutations never fall back to WSL.
- POSIX utilities such as `find`, `head`, and `tail` use raw WSL executables;
  a same-named Windows system tool is not treated as semantically compatible.
- Drive CWD mapping, complete 32-bit Windows exit-code propagation,
  stdout/stderr, Ctrl+C, child processes, and lock release have automated
  process-contract coverage.
- WSL1 and WSL2 use an attest-then-permit launch handshake. A target cannot
  begin until the parent has observed its cancellation boundary, so an
  immediate Ctrl+C cannot leave a command starting in the background.
- WSL1 keeps a Linux supervisor alive after the permit. Normal return is
  accepted only when that supervisor has stopped same-process-group
  descendants, published an installation-ID-bound completion record, and its
  status matches the Windows proxy. A missing or contradictory completion
  resets only the revalidated dedicated WSL1 distro before the mutex is
  released.
- WSL2 cancellation state lives under a nonce-named file in a non-symlink,
  user-owned `0700` runtime directory; token files are regular, user-owned
  `0600` files. A launcher removes another token only after proving its Linux
  process group is gone; token age alone is never a deletion condition.
- WSL2 completion is identity-attested after the launcher has reaped or stopped
  every member of its process group. Cancellation continues inside Linux even
  if the Windows `wsl.exe` proxy exits first, and a missing token is not treated
  as proof of cleanup without that completion attestation.
- XUVA is a foreground command dispatcher, not a daemon launcher. A command
  cannot detach same-process-group children past XUVA's return. Deliberately
  creating a separate Linux session (for example with `setsid`) crosses the
  supported supervision boundary and must be managed by an external service
  manager.
- WSL use requires an explicit verified provider and path mapping. WSL1 and
  WSL2 are measured routes, not a default performance claim.
- Existing Windows and WSL tool installations can be diagnosed on demand.
  Installation is always separately planned and confirmed.

When invoking the Windows dispatcher from a WSL shell, use the provided shim so
the originating distro, physical CWD, Windows-mounted CWD when available, and
structured argv are retained:

```sh
export XUVA_WINDOWS_EXE=/mnt/c/tools/xuva.exe
sh /mnt/c/tools/xuva-wsl.sh go version
```

Set `XUVA_WSL_EXTRA_PATH` only when a WSL-only tool lives outside that distro's
normal `PATH`. The shim is required because WSL does not forward arbitrary
Linux environment variables to Windows `.exe` processes.

Cross-host children start from a clean environment. XUVA forwards a small
safe set (`CI`, color controls, `TERM`, and `RUST_BACKTRACE`), boolean
`*_RUN_*` feature gates such as `XPDE_RUN_TRAINING_E2E=1`, and names explicitly
listed in the comma-separated `XUVA_ENV_ALLOWLIST`. Credential-like names are
rejected even when they resemble a feature gate or appear in the allowlist.

Local aggregate metrics are **off by default**. Enable them explicitly with
`XUVA_METRICS=on`. XUVA stores only route, command family, token totals,
duration, and exit code; command arguments, project paths, parse input, and
error text are never persisted. Scratch databases are private and removed by
an RAII guard, including their WAL/SHM sidecars. The aggregate ledger retains
at most the newest 10,000 invocations.

Command family is a bounded lowercase executable basename; only allowlisted Git
subcommands may be appended. Metrics-enabled raw fast paths are included in the
ledger, while metrics-off fast paths remain ledger- and scratch-free.

This ledger is separate from adaptive calibration. A native calibration sample
may use a private temporary RTK counter database, but it persists only bounded
opaque evidence in `calibration-v3.json` and removes the temporary database at
the end of the invocation.

Use `xuva metrics status` to inspect the privacy contract and totals, or
`xuva metrics purge` to delete all local metrics artifacts. See the full
[local metrics privacy contract](docs/METRICS_PRIVACY.md).

Build identity is inspectable without WSL or provider discovery:

```powershell
xuva --version --verbose
```

It reports the package version, source commit, target, build profile, and
provenance channel embedded at build time.

## Documentation

| Topic | Reference |
| --- | --- |
| Canonical product, security, architecture, CLI UX, and performance rules | [Product and engineering guideline](docs/PRODUCT_ENGINEERING_GUIDELINE.md) |
| XUVA command migration and compatibility | [Migration notes](docs/XUVA_MIGRATION.md) |
| Routing, configuration, and local accounting | [XUVA contract](docs/XUVA.md) |
| Public benchmark comparison | [P20 comparison](docs/BENCHMARK_COMPARISON_P20.md) |
| Public external-corpus benchmark | [P21 ripgrep evidence](docs/P21_PUBLIC_RIPGREP_BENCHMARK.md) |
| Public Node-package benchmark | [P21 TypeScript `npm run` evidence](docs/P21_PUBLIC_TYPESCRIPT_NPM_BENCHMARK.md) |
| Public pytest capability | [P21 pytest evidence gap](docs/P21_PUBLIC_PYTEST_CAPABILITY.md) |
| Full native Windows, WSL1, and WSL2 matrix | [P18 core matrix](docs/BENCHMARK_CORE_MATRIX_P18_2026-07-25.md) |
| Provider discovery, mapping, and execution | [Provider documentation index](docs/README.md#cross-host-providers-and-setup) |
| Fresh-machine tokenizer dependency | [Runtime dependencies](docs/DEPENDENCIES.md) |
| Claude Code adapter | [Agent integration](docs/AGENT_INTEGRATION.md) |
| Controlled Windows/WSL evidence | [Self-hosted WSL CI](docs/SELF_HOSTED_WSL_CI.md) |
| Artifact provenance and signing readiness | [Release provenance](docs/RELEASE_PROVENANCE.md) |
| Installation, rollback, and uninstall | [Packaging/recovery contract](tests/packaging-contract.ps1) |
| Full local alpha verification | [P20 release gate](docs/RELEASE_GATE_P20.md) |
| Alpha delivery history | [Milestone documents](docs/README.md#release-and-project-history) |

## Verify from source

```powershell
cargo fmt --check
cargo test
cargo clippy -- -D warnings
.\scripts\verify-release.ps1
```

The release gate covers Rust checks, process contracts, tokenizer bootstrap,
packaging/recovery, setup readiness, exact Windows/WSL provider preflight,
command-surface parity, and crate hygiene.

## License and upstream boundary

XUVA is Apache-2.0 licensed to match RTK. It is not an official RTK package
and remains `publish = false`. Potential upstream contributions must target
RTK's `develop` branch, be scoped and tested independently, and comply with
its contributor requirements.
