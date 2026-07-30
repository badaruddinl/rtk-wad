# XUVA adaptive routing contract

`xuva` is the canonical Windows command for this project. It is an adaptive
dispatcher; it is not a shell wrapper and does not stringify or re-parse the
arguments it receives.

## Canonical command

`xuva` is the primary executable, installer target, and supported command
surface. There is no legacy launcher in the current release line; WSL1 and
WSL2 remain provider routes rather than separate binaries.

## Route selection

`xuva` resolves exactly one route before it starts a child process:

| Route | Auto-selection rule | Execution |
| --- | --- | --- |
| `raw` | Windows-worktree Git, Windows-native tools, or POSIX utilities with a verified WSL executable | Native executable exactly once on the owning host. |
| `native-rtk` | Verified structured RTK command families with a native adapter | Stock Windows RTK with structured argv. |
| `wsl1` | Explicit WSL1 backend or the only compatible provider | Isolated WSL1 provider. |
| `wsl2` | Preferred verified Linux provider on the configured distro | Existing WSL2 provider with structured argv. |

This policy deliberately does not fall back after a child process starts or
returns a non-zero exit status. A failure therefore has the same observable
effect as the selected backend and cannot replay a mutating command. Use the
following command to inspect a decision without execution:

```powershell
xuva --explain-route git log -1
xuva --route wsl1 --explain-route proxy /usr/bin/printf '%s'
```

For an `auto` decision only, a missing native RTK executable is an exception:
XUVA falls back to WSL1 only when Windows reports `NotFound` while starting the
native process. No child has started in that case. An explicit `native-rtk`
route always fails directly if its configured executable is unavailable.

`npm`, `npx`, `pnpm`, `go`, and `dotnet` are validated Windows raw routes while WSL1 has
no matching toolchain. They are still eligible for future evidence-backed native RTK
promotion; raw is selected now because it executes once and avoids an otherwise
guaranteed WSL tool-not-found failure.

`dart` and `flutter` are XUVA-owned Windows tool shims, not upstream RTK command
families. They likewise execute their `.bat` launchers once with structured argv;
this keeps Flutter Windows workflows out of the WSL shell path.

`XUVA_ROUTE` sets the default route (`auto`, `raw`, `native-rtk`, `wsl1`, or
`wsl2`). A leading `--route` option has higher precedence for a single
invocation. `XUVA_NATIVE_RTK_PATH` selects the stock native RTK executable;
it defaults to `rtk.exe` on `PATH`.

Git in a Windows worktree is pinned to Git for Windows. This includes
read-only commands, NTFS mutations such as `commit`, and network mutations such
as `push`; WSL is not a fallback for a Windows Git mutation. This preserves
Git's object-store permissions, credential provider, proxy, and Windows DNS
configuration. `xuva --explain-route ...` reports that ownership explicitly,
and `xuva doctor git --json` includes the routing-health advisory.

## Evidence-backed decisions

The static policy is safe when no evidence exists. A local three-way benchmark
can additionally install route evidence:

```powershell
xuva policy import .\flowpeek.route-policy.json
xuva policy show
```

Import merges evidence by its command key and atomically replaces only a key
that was measured again. A later Node or Cargo benchmark therefore preserves
existing Git and search evidence in the same local policy file.

For at least five samples, the default `balanced` objective selects native RTK
when measured token saving is 25% or more. The comparison uses end-to-end XUVA
latency, including dispatcher and local-accounting cost. When measured saving
is below that threshold and raw execution is no slower, XUVA selects raw
execution. `XUVA_POLICY_OBJECTIVE=latency` instead chooses the lower median;
`tokens` prefers any positive measured saving. The objective is included in the
opaque evidence context, so changing objectives invalidates incompatible local
policy/calibration evidence. This applies only to the
verified read-only Git allowlist, `rg`, verified Cargo operations, the exact
read-only `npm run` listing form, and the exact `go test ./...` form. A policy file can never make a Git mutation
or `npm run <script>` adaptive, nor can it cause a command to run twice. The
local policy is read-only during normal execution and can be overridden for
testing with `XUVA_POLICY_PATH`.

## Local adaptive calibration (P10)

When an eligible command has no imported benchmark-policy entry, XUVA may build
small local evidence across the user's normal invocations. This is a
deterministic state machine, not a trained model and not an in-process command
replay:

1. The first successful natural invocation uses native RTK.
2. The second successful natural invocation uses raw execution.
3. The third successful natural invocation uses native RTK again.
4. The fourth invocation follows a provisional choice based on the two native
   observations, one raw observation, and RTK's aggregate token counters.
5. If needed, one further natural raw observation is collected before the
   route becomes stable. The stable decision keeps at most five recent samples
   for each route.

The local selector chooses native RTK at 25% or greater measured token saving;
below that threshold it chooses the lower median end-to-end latency. The timing
includes dispatcher and local metrics overhead. An imported policy entry always
has precedence over local calibration. Failed commands and a native process
that falls back to WSL are not calibration evidence.

Calibration is restricted to `rg`, the verified read-only Git allowlist, the
exact read-only `npm run` listing form, and the exact `go test ./...` form. It
excludes mutations, unknown commands, WSL paths, explicit routes, and Cargo
because its ordinary commands can write build artifacts. The state stores only
a deterministic 64-bit signature, a safe command category, route timings, and
aggregate native RTK token counts. It never stores the command arguments or
its output.

```powershell
xuva --explain-route rg -n 'needle' src
xuva calibration show
```

`--explain-route` is read-only: it reports the next route but does not advance
the calibration cycle. See
[`LOCAL_ADAPTIVE_CALIBRATION_P10.md`](LOCAL_ADAPTIVE_CALIBRATION_P10.md) for
the full safety and validation contract.

## On-demand provider discovery (PD1)

`xuva resolve go` and `xuva doctor go` inspect existing Windows and WSL
Go providers without changing command routing or installing anything. The
five-minute local cache is per tool and can be bypassed with `--refresh`. See
[`PROVIDER_DISCOVERY_PD1.md`](PROVIDER_DISCOVERY_PD1.md) for the exact scope,
cache content, and deliberate cross-host limitations.

A normal dispatch or `resolve` reuses the bounded cache without spawning Git,
WSL identity, or version probes. Discovery checks the configured distro first
and stops after the first sufficient provider. `doctor`, `--refresh`, setup,
and explicit provider execution perform the complete inventory and version
inspection. Repository revisions are intentionally not a provider-cache key.

P13 validates a project's actual cross-host directory in both directions using
the structured `wsl.exe --exec wslpath` form plus a target-host directory
probe. This is still diagnostic-only; see
[`BIDIRECTIONAL_PROVIDER_MAPPING_P13.md`](BIDIRECTIONAL_PROVIDER_MAPPING_P13.md).

P14 adds `xuva provider exec <tool> [--candidate <index>] -- <args...>` for
one verified provider execution without shell reconstruction or post-start
fallback. It remains explicit while P15 classifies the command surface; see
[`PROVIDER_EXECUTION_ENGINE_P14.md`](PROVIDER_EXECUTION_ENGINE_P14.md).

P15 embeds the complete RTK `0.43.0` command manifest. `xuva surface
--json` reports all 69 families and their conservative route classes, while a
runtime contract compares that inventory with the live WSL RTK help output; see
[`COMMAND_SURFACE_PARITY_P15.md`](COMMAND_SURFACE_PARITY_P15.md).

P16 binds adaptive policy and local calibration to the current manifest plus
an opaque Windows adapter-context signature. Use `xuva policy context`
before creating an importable benchmark policy; see
[`ADAPTIVE_DECISION_HARDENING_P16.md`](ADAPTIVE_DECISION_HARDENING_P16.md).

PD3 uses a complete verified Go provider for automatic execution only when
Windows Go is unavailable. A missing safe provider exits `127` before a child
starts; installation remains disabled. See
[`PROVIDER_EXECUTION_PD3.md`](PROVIDER_EXECUTION_PD3.md).

PD4 adds `xuva setup go [--json] [--refresh]`, which renders a local setup
plan only. It can show a narrowly scoped Windows `winget` command when native
RTK exists, but it does not execute the command or install anything. See
[`ASSISTED_SETUP_PD4.md`](ASSISTED_SETUP_PD4.md).

PD5 adds an opt-in apply boundary: `xuva setup go --apply` re-renders a
fresh plan, while only `xuva setup go --apply --confirm` can execute its
single structured `winget` command. `--status` reports the local journal and
`--recover` re-discovers providers without replaying an installer. See
[`OPT_IN_SETUP_PD5.md`](OPT_IN_SETUP_PD5.md).

PD6 adds the local-only readiness gate
[`setup-readiness-contract.ps1`](../tests/setup-readiness-contract.ps1) and
the [operational freeze](SETUP_OPERATIONAL_FREEZE_PD6.md). The gate never calls
`winget` and is intended to run before an alpha release decision.

P17 makes `doctor <tool>` and `setup <tool>` available for every safe provider
name. Generic setup is diagnostic-only: it returns `ready` for an existing
verified provider or `blocked` with no proposed installer. It rejects apply,
confirmation, status, and recovery flags, and never creates the Go setup
journal. See [`GENERIC_SETUP_DIAGNOSIS_P17.md`](GENERIC_SETUP_DIAGNOSIS_P17.md).

P18 adds a local benchmark-matrix preflight. It validates the native Windows,
WSL1, and WSL2 RTK providers against the embedded command manifest before a
benchmark may claim latency or token evidence. An absent provider blocks that
backend's evidence; XUVA never installs or substitutes it. See
[`BENCHMARK_MATRIX_P18.md`](BENCHMARK_MATRIX_P18.md).

## Cache-aware adaptive routing (P7)

Automatic Windows Go execution uses a lightweight local provider cache. A fresh
Windows Go result no longer probes WSL unless Windows Go is unavailable; complete
cross-host probing remains available to `resolve`, `doctor`, and setup. Cache
state never by itself promotes a route. An imported repeated-run policy may
select native RTK for verified read-only workloads when it is faster or meets
the token-saving threshold. See
[`ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md`](ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md).

## Shell and argv safety

Use structured RTK commands such as `rg`, `grep`, `read`, `files`, and `diff`.
POSIX utilities such as `find`, `head`, `tail`, `sed`, `awk`, `ls`, `tree`, and
`wc` use a raw WSL executable so GNU/POSIX flags are not handed to an
incompatible Windows system tool or a narrower output adapter. Commands that rely on
the upstream Windows `run` parser or a single-string `proxy` form are not
auto-routed to stock Windows RTK. They use WSL1 until their native contracts
are independently verified.

RTK meta commands such as `smart`, `proxy`, `rewrite`, and `hook` are
adapter-owned subcommands rather than operating-system executables. Adaptive
routing keeps their static RTK route and never asks the generic provider
resolver to discover a fictitious executable with the same name.

`xuva` does not provide an implicit shell mode. If a workflow genuinely
requires shell syntax, invoke the required shell explicitly and keep it on a
forced WSL route, for example:

```powershell
xuva --route wsl1 proxy /bin/sh -c 'printf "%s" "$HOME"'
```

If a shell operator such as `&&` is passed as the command, XUVA exits with an
actionable shell-syntax diagnostic. Operators inside another command's argv
remain literal. XUVA never joins, quotes, or re-parses the user's argv.

Windows native executables receive direct argv. `.cmd` and `.bat` providers
necessarily cross the Windows batch interpreter boundary: percent,
exclamation, caret, quotes, empty strings, and trailing backslashes are
preserved by the tested launcher contract, while CR/LF arguments are rejected
before a process starts. POSIX command families (`find`, `head`, `tail`, `sed`,
`awk`, `ls`, `tree`, and `wc`) require a raw Linux executable and never accept a
same-named Windows utility as a semantic substitute.

## Cross-host environment

Cross-host execution is determined from explicit invocation origin, not from
whether a drive mapping happens to exist. WSL-to-Windows providers receive an
isolated Windows environment and a CWD only when the bridge supplies an exact
drive mapping or a matching `\\wsl.localhost\<distro>\...`/`\\wsl$` UNC
mapping. Same-host execution may inherit its native environment.

WSL execution retains a clean `env -i` boundary and adds only structured,
reviewable assignments. The built-in safe set is `CI`, `COLORTERM`,
`FORCE_COLOR`, `NO_COLOR`, `RUST_BACKTRACE`, and `TERM`. Boolean `*_RUN_*`
feature gates are forwarded automatically when their value is exactly `0` or
`1`; this includes `XPDE_RUN_TRAINING_E2E=1`.

Additional non-secret names may be listed with a comma-separated
`XUVA_ENV_ALLOWLIST`. Names must use POSIX identifier syntax. Credential-like
names containing markers such as `TOKEN`, `SECRET`, `PASSWORD`, `CREDENTIAL`,
`COOKIE`, `PRIVATE_KEY`, `ACCESS_KEY`, or `AUTH` are refused. Environment
assignments stay before the executable in the structured plan; no shell command
is reconstructed.

## Update diagnosis

`xuva self-update --check` queries stable `vMAJOR.MINOR.PATCH` tags through Git
for Windows with a ten-second timeout and reports `up-to-date` or
`update-available`. It does not install or overwrite anything. `xuva
self-update` prints the reviewed release/installer workflow, and failures point
to Windows Git, DNS, proxy, or credential health rather than being routed as an
external `self-update` command.

## Optional benchmark tokenizer

The canonical `xuva` installer has no Python dependency. `tiktoken==0.12.0`
is an optional private XUVA benchmark environment, installed only with
`scripts/install.ps1 -InstallTokenizer`. This keeps reproducible `o200k_base`
measurements available without making a dispatcher depend on Python. It never
installs into the user's global Python environment, and a failed optional setup
leaves an active launcher unchanged. The dependency record and upgrade contract
are in [runtime dependencies](DEPENDENCIES.md).

## Cross-host provider boundary

Verified Windows-to-WSL and WSL-to-Windows provider execution, cache limits,
and the on-demand dependency boundary are documented in
[P19 cross-host resolution](CROSS_HOST_ON_DEMAND_P19.md). Generic setup remains
diagnostic-only; cross-host execution is always explicit through `provider
exec` after XUVA validates the candidate host and project path.

## Local token savings ledger

`xuva gain` and its `stats` compatibility alias show local **RTK-measured
token accounting**, not an estimate for every invocation. XUVA creates a unique temporary RTK tracker
database for an individual routed invocation, reads only aggregate counters,
then removes the temporary database and its WAL sidecars. The persistent local
ledger contains timestamp, route, command family, aggregate token counts,
elapsed time, exit code, and whether upstream RTK recorded a measurement. It
does not retain command arguments or command output.

The ledger is stored under `%LOCALAPPDATA%\xuva\metrics-v1.sqlite`. Set
`XUVA_STATE_DIR` only for an isolated test or benchmark ledger; it overrides
that complete XUVA state root without changing the child command's Windows
profile or caches. A
command-free RTK schema template is stored beside it so each scratch database
can be prepared without a first-use migration. Scratch databases live under
`%LOCALAPPDATA%\xuva\scratch` and stale entries older than 24 hours are
removed at the next invocation. Both locations are on the Windows system drive,
so a repository may stay on exFAT, FAT, a network drive, or another non-NTFS
source volume. The source filesystem is never used for the ledger or WSL runtime
state.

Raw routes are included only as invocation counts and are marked unmeasured;
their input, output, and avoided-token fields remain zero. The summary labels
its measured scope explicitly and never turns raw output into a fabricated
token-saving estimate by replaying a command. A positive value means only that
the selected RTK tracker reported avoided tokens; it is not a claim that XUVA
made every invocation faster or that it measured raw-route token savings.

## Diagnostics

```powershell
xuva --adapter-info
xuva gain
xuva --explain-route rg 'pattern with spaces' .
```

The adaptive path does not call `wsl.exe --list` or probe the filesystem before
each command. WSL version diagnostics remain explicit through the legacy bridge
commands.

For deterministic fixtures or intentionally provisioned Linux tools, set
`XUVA_WSL_EXTRA_PATH` to a colon-separated list of absolute Linux directories.
Relative entries and empty segments are rejected before WSL starts; the default
child PATH remains unchanged.
