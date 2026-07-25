# RTK-WAD adaptive routing contract

`rtk-wad` is the canonical Windows command for this project. It is an adaptive
dispatcher; it is not a shell wrapper and does not stringify or re-parse the
arguments it receives.

## Profiles

| Command | Contract |
| --- | --- |
| `rtk-wad` | Adaptive Windows dispatcher. |
| `rtk-wsl` | Compatibility launcher that keeps the original WSL-oriented behavior. |
| `rtk-wsl1` | Explicit isolated WSL1 launcher. |

The executable name chooses a profile. Environment variables still override the
explicit WSL profile configuration where documented.

## Route selection

`rtk-wad` resolves exactly one route before it starts a child process:

| Route | Auto-selection rule | Execution |
| --- | --- | --- |
| `raw` | Git mutation (`commit`, `push`, `reset`, and similar) | Native executable exactly once. |
| `native-rtk` | Verified structured RTK command families and read-only Git | Stock Windows RTK with structured argv. |
| `wsl1` | Linux path, WSL working directory, or no verified native adapter | Isolated WSL1 RTK. |
| `wsl2` | Explicit only during alpha | Existing WSL2 RTK bridge. |

This policy deliberately does not fall back after a child process starts or
returns a non-zero exit status. A failure therefore has the same observable
effect as the selected backend and cannot replay a mutating command. Use the
following command to inspect a decision without execution:

```powershell
rtk-wad --explain-route git log -1
rtk-wad --route wsl1 --explain-route proxy /usr/bin/printf '%s'
```

For an `auto` decision only, a missing native RTK executable is an exception:
WAD falls back to WSL1 only when Windows reports `NotFound` while starting the
native process. No child has started in that case. An explicit `native-rtk`
route always fails directly if its configured executable is unavailable.

`npm`, `npx`, `pnpm`, `go`, and `dotnet` are validated Windows raw routes while WSL1 has
no matching toolchain. They are still eligible for future evidence-backed native RTK
promotion; raw is selected now because it executes once and avoids an otherwise
guaranteed WSL tool-not-found failure.

`dart` and `flutter` are WAD-owned Windows tool shims, not upstream RTK command
families. They likewise execute their `.bat` launchers once with structured argv;
this keeps Flutter Windows workflows out of the WSL shell path.

`RTK_WAD_ROUTE` sets the default route (`auto`, `raw`, `native-rtk`, `wsl1`, or
`wsl2`). A leading `--route` option has higher precedence for a single
invocation. `RTK_WAD_NATIVE_RTK_PATH` selects the stock native RTK executable;
it defaults to `rtk.exe` on `PATH`.

## Evidence-backed decisions

The static policy is safe when no evidence exists. A local three-way benchmark
can additionally install route evidence:

```powershell
rtk-wad policy import .\flowpeek.route-policy.json
rtk-wad policy show
```

Import merges evidence by its command key and atomically replaces only a key
that was measured again. A later Node or Cargo benchmark therefore preserves
existing Git and search evidence in the same local policy file.

For at least five samples, WAD selects native RTK when measured token saving is
25% or more. The comparison uses end-to-end WAD latency, including dispatcher
and local-accounting cost. When measured saving is below that threshold and raw
execution is no slower, WAD selects raw execution. This applies only to the
verified read-only Git allowlist, `rg`, verified Cargo operations, the exact
read-only `npm run` listing form, and the exact `go test ./...` form. A policy file can never make a Git mutation
or `npm run <script>` adaptive, nor can it cause a command to run twice. The
local policy is read-only during normal execution and can be overridden for
testing with `RTK_WAD_POLICY_PATH`.

## Local adaptive calibration (P10)

When an eligible command has no imported benchmark-policy entry, WAD may build
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
rtk-wad --explain-route rg -n 'needle' src
rtk-wad calibration show
```

`--explain-route` is read-only: it reports the next route but does not advance
the calibration cycle. See
[`LOCAL_ADAPTIVE_CALIBRATION_P10.md`](LOCAL_ADAPTIVE_CALIBRATION_P10.md) for
the full safety and validation contract.

## On-demand provider discovery (PD1)

`rtk-wad resolve go` and `rtk-wad doctor go` inspect existing Windows and WSL
Go providers without changing command routing or installing anything. The
five-minute local cache is per tool and can be bypassed with `--refresh`. See
[`PROVIDER_DISCOVERY_PD1.md`](PROVIDER_DISCOVERY_PD1.md) for the exact scope,
cache content, and deliberate cross-host limitations.

P13 validates a project's actual cross-host directory in both directions using
the structured `wsl.exe --exec wslpath` form plus a target-host directory
probe. This is still diagnostic-only; see
[`BIDIRECTIONAL_PROVIDER_MAPPING_P13.md`](BIDIRECTIONAL_PROVIDER_MAPPING_P13.md).

P14 adds `rtk-wad provider exec <tool> [--candidate <index>] -- <args...>` for
one verified provider execution without shell reconstruction or post-start
fallback. It remains explicit while P15 classifies the command surface; see
[`PROVIDER_EXECUTION_ENGINE_P14.md`](PROVIDER_EXECUTION_ENGINE_P14.md).

P15 embeds the complete RTK `0.43.0` command manifest. `rtk-wad surface
--json` reports all 69 families and their conservative route classes, while a
runtime contract compares that inventory with the live WSL RTK help output; see
[`COMMAND_SURFACE_PARITY_P15.md`](COMMAND_SURFACE_PARITY_P15.md).

P16 binds adaptive policy and local calibration to the current manifest plus
an opaque Windows adapter-context signature. Use `rtk-wad policy context`
before creating an importable benchmark policy; see
[`ADAPTIVE_DECISION_HARDENING_P16.md`](ADAPTIVE_DECISION_HARDENING_P16.md).

PD3 uses a complete verified Go provider for automatic execution only when
Windows Go is unavailable. A missing safe provider exits `127` before a child
starts; installation remains disabled. See
[`PROVIDER_EXECUTION_PD3.md`](PROVIDER_EXECUTION_PD3.md).

PD4 adds `rtk-wad setup go [--json] [--refresh]`, which renders a local setup
plan only. It can show a narrowly scoped Windows `winget` command when native
RTK exists, but it does not execute the command or install anything. See
[`ASSISTED_SETUP_PD4.md`](ASSISTED_SETUP_PD4.md).

PD5 adds an opt-in apply boundary: `rtk-wad setup go --apply` re-renders a
fresh plan, while only `rtk-wad setup go --apply --confirm` can execute its
single structured `winget` command. `--status` reports the local journal and
`--recover` re-discovers providers without replaying an installer. See
[`OPT_IN_SETUP_PD5.md`](OPT_IN_SETUP_PD5.md).

PD6 adds the local-only readiness gate
[`setup-readiness-contract.ps1`](../tests/setup-readiness-contract.ps1) and
the [operational freeze](SETUP_OPERATIONAL_FREEZE_PD6.md). The gate never calls
`winget` and is intended to run before an alpha release decision.

## Cache-aware adaptive routing (P7)

Automatic Windows Go execution uses a lightweight local provider cache. A fresh
Windows Go result no longer probes WSL unless Windows Go is unavailable; complete
cross-host probing remains available to `resolve`, `doctor`, and setup. Cache
state never by itself promotes a route. An imported repeated-run policy may
select native RTK for verified read-only workloads when it is faster or meets
the token-saving threshold. See
[`ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md`](ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md).

## Shell and argv safety

Use structured RTK commands such as `git`, `rg`, `grep`, `find`, `ls`, `tree`,
`read`, `files`, and `diff`. They are eligible for the native route because
their RTK implementations receive distinct arguments. Commands that rely on
the upstream Windows `run` parser or a single-string `proxy` form are not
auto-routed to stock Windows RTK. They use WSL1 until their native contracts
are independently verified.

`rtk-wad` does not provide an implicit shell mode. If a workflow genuinely
requires shell syntax, invoke the required shell explicitly and keep it on a
forced WSL route, for example:

```powershell
rtk-wad --route wsl1 proxy /bin/sh -c 'printf "%s" "$HOME"'
```

## Local token savings ledger

`rtk-wad gain` and its `stats` compatibility alias show aggregate token savings
in a native-RTK-style summary. WAD creates a unique temporary RTK tracker
database for an individual routed invocation, reads only aggregate counters,
then removes the temporary database and its WAL sidecars. The persistent local
ledger contains timestamp, route, command family, aggregate token counts,
elapsed time, exit code, and whether upstream RTK recorded a measurement. It
does not retain command arguments or command output.

The ledger is stored under `%LOCALAPPDATA%\rtk-wad\metrics-v1.sqlite`. Set
`RTK_WAD_STATE_DIR` only for an isolated test or benchmark ledger; it overrides
that complete WAD state root without changing the child command's Windows
profile or caches. A
command-free RTK schema template is stored beside it so each scratch database
can be prepared without a first-use migration. Scratch databases live under
`%LOCALAPPDATA%\rtk-wad\scratch` and stale entries older than 24 hours are
removed at the next invocation. Both locations are on the Windows system drive,
so a repository may stay on exFAT, FAT, a network drive, or another non-NTFS
source volume. The source filesystem is never used for the ledger or WSL runtime
state.

Raw routes are included as invocations but are initially marked unmeasured;
WAD never fabricates a token-saving estimate by rerunning a command.

## Diagnostics

```powershell
rtk-wad --adapter-info
rtk-wad gain
rtk-wad --explain-route rg 'pattern with spaces' .
```

The adaptive path does not call `wsl.exe --list` or probe the filesystem before
each command. WSL version diagnostics remain explicit through the legacy bridge
commands.

For deterministic fixtures or intentionally provisioned Linux tools, set
`RTK_WSL_EXTRA_PATH` to a colon-separated list of absolute Linux directories.
Relative entries and empty segments are rejected before WSL starts; the default
child PATH remains unchanged.
