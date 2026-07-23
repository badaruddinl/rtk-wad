# Alpha stabilization record

This document records outcomes only; it deliberately avoids raw command logs.

## Current gates

| Gate | Evidence | Status |
| --- | --- | --- |
| Portable configuration | Validated environment contract; `RTK_WSL_CWD` covers custom mounts | Pass |
| Argument and process contract | Rust integration tests for literals, Unicode, stdio, exit codes, stdin | Pass |
| Ctrl+C / Ctrl+Break | Windows handler forwards SIGINT to the dedicated Linux process group; lock-release regression passes | Pass |
| Packaging and recovery | Isolated PowerShell contract using a temporary destination | Pass |
| Rust quality gate | fmt, clippy, 10 tests, release build, package archive audit | Pass |
| Long-running dogfood | Two concise repository cycles, recording only failures/outliers/fallbacks | Evidence collected |

## Decision boundary

The cancellation contract is green. Apache-2.0 and the public companion repository
remain provisional local publication metadata, not an upstream RTK contribution
decision.

## Cancellation contract

Each launch creates a unique token under `/tmp`. The fixed Linux launcher script
starts `flock` and RTK in a dedicated `setsid` session, records that process-group
leader in the token, and removes the token when the command exits. The Windows
console handler consumes Ctrl+C or Ctrl+Break, then issues a separate structured
WSL invocation that validates the numeric token and sends SIGINT to only that
process group.

This avoids shell reconstruction, does not derive identity by launching a helper
process for every command, and never uses `wsl --terminate`; other commands and
other processes in the distro remain untouched. The process-contract regression
starts two contending commands, sends Ctrl+Break to the first Windows process group,
and proves that the waiting command proceeds after the lock is released.

## Dogfooding evidence (2026-07-24)

| Cycle | Work performed | Failure / fallback | Observation |
| --- | --- | --- | --- |
| `rtk-wsl` | Git status and README read using the release binary | None | 14.3 s wall time; treat as a WSL startup outlier, not a performance claim |
| Flowpeek | Git status and README discovery using the same release binary | None | 10.7 s wall time; treat as a WSL startup outlier, not a performance claim |

The package archive contained source, tests, scripts, documentation, and Cargo
metadata only. It did not contain `target/`, workstation configuration, or raw
dogfooding logs.

See `docs/ALPHA_RELEASE_CHECKLIST.md` for the remaining freeze and publication
decisions.
