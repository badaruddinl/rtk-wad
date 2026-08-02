# P10 local adaptive calibration

## Purpose

P10 supplies a bounded local route-selection foundation for XUVA when no
imported repeated-run benchmark policy is available for a command. It improves
on a static preference without turning ordinary commands into benchmark
replays.

The mechanism is deterministic. It is not a machine-learning model, does not
send data anywhere, and does not inspect or retain command output.

## Safety boundary

Only these read-only contracts can enter local calibration:

| Command form | Reason |
| --- | --- |
| `rg ...` | Search-only adapter contract. |
| Verified read-only Git subcommands | Existing structured Git contract. |
| Exact `npm run` | Lists scripts without running one. |
| Exact `go test ./...` | Existing token-first Go benchmark contract. |

The following always stay outside it: explicit `--route`, any WSL path or WSL
working directory, Git mutations, unknown commands, `npm run <script>`, and
Cargo commands. Cargo is deliberately excluded because normal Cargo activity
can write the target directory and caches.

An imported route-policy entry has higher precedence than local calibration.
No selected route is retried after a child starts. A missing native RTK binary
may take the pre-start WSL fallback already defined by WAD, but that fallback
is not recorded as native calibration evidence.

## Natural-invocation sequence

The command is never duplicated inside one WAD invocation. For the same
project and exact argument shape, the initial cycle is:

| Natural invocation | Route | State after success |
| --- | --- | --- |
| 1 | Native RTK | Candidate: native sample 1 |
| 2 | Raw | Candidate: raw sample 1 |
| 3 | Native RTK | Provisional evidence |
| 4 | Provisional selected route | Stable if both routes now have two samples |
| 5, only when required | Raw validation | Stable evidence |

The fourth command is useful immediately, but WAD does not describe a route
as stable until it has at least two successful observations for both routes.
This avoids falsely treating one unusually slow raw invocation as a permanent
result. Later observations retain only the five most recent samples per route.

## Decision rule

XUVA measures end-to-end elapsed time, including dispatcher and
local-accounting cost. Each retained native sample stores its own elapsed time,
input-token count, and saved-token count. The route decision derives token
saving from the same bounded rolling sample window as latency:

```text
native_savings_percent =
    sum(recent_native_samples.saved_tokens)
    / sum(recent_native_samples.input_tokens)
    * 100
```

The selection is intentionally simple and auditable:

1. At least 25% measured native RTK saving selects `native-rtk`.
2. Otherwise, the lower median latency selects the route.
3. With insufficient latency evidence, the candidate sequence supplies the
   next required natural observation.

Raw calibration stores only elapsed time. XUVA does not tokenize raw output or
manufacture a raw token estimate. The opt-in `gain` ledger is separate: it
records the invocation only when `XUVA_METRICS=on`.

## Local state and privacy

The current schema supersedes the original v1/v2 locations with
`%LOCALAPPDATA%\xuva\calibration-v3.json`, or the matching isolated
`XUVA_STATE_DIR` root. It is atomically replaced and contains:

- a deterministic 64-bit FNV-1a signature of project path plus arguments;
- a safe command category;
- bounded raw latency samples; and
- bounded native samples containing latency plus input/saved-token counters.

It does **not** contain the project path, argument text, command output,
environment, or raw-output token counts. `xuva calibration show` exposes
only the category, opaque signature, phase, route, sample counts, and native
token-saving percentage.

Calibration is enabled by default and is independent from aggregate metrics.
With `XUVA_METRICS=off`, an eligible native sample may create a private
temporary RTK counter database, but it is removed after the invocation and no
metrics ledger is created. Set `XUVA_CALIBRATION=off` to skip calibration state
reads, temporary measurement, and writes entirely.

## Validation gates

P10 adds unit coverage for the candidate → provisional → stable transition,
the default balanced 25% token threshold, objective-specific evidence context,
safe-command exclusion, and non-revealing signature
behavior. The release gate remains:

```powershell
cargo fmt --check
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo package
```

Runtime validation uses an isolated `XUVA_STATE_DIR`, invokes a safe local
search naturally through the sequence, and confirms that `--explain-route`
does not write state.
