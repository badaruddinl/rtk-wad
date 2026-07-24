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

`npm` and `npx` are validated Windows raw routes while WSL1 has no Node
toolchain. They are still eligible for future evidence-backed native RTK
promotion; raw is selected now because it executes once and avoids an otherwise
guaranteed WSL tool-not-found failure.

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
verified read-only Git allowlist, `rg`, verified Cargo operations, and the exact
read-only `npm run` listing form. A policy file can never make a Git mutation
or `npm run <script>` adaptive, nor can it cause a command to run twice. The
local policy is read-only during normal execution and can be overridden for
testing with `RTK_WAD_POLICY_PATH`.

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

The ledger is stored under `%LOCALAPPDATA%\rtk-wad\metrics-v1.sqlite`. A
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
