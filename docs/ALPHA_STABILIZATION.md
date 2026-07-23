# Alpha stabilization record

This document records outcomes only; it deliberately avoids raw command logs.

## Current gates

| Gate | Evidence | Status |
| --- | --- | --- |
| Portable configuration | Validated environment contract; `RTK_WSL_CWD` covers custom mounts | Pass |
| Argument and process contract | Rust integration tests for literals, Unicode, stdio, exit codes, stdin | Pass |
| Ctrl+C / Ctrl+Break | Retained ignored regression probe; WSL does not forward Ctrl+Break to `--exec` child | Blocked |
| Packaging and recovery | Isolated PowerShell contract using a temporary destination | Pass |
| Rust quality gate | fmt, clippy, test, release build, package archive audit | Pass (one Ctrl+Break test ignored) |
| Long-running dogfood | Two concise repository cycles, recording only failures/outliers/fallbacks | Evidence collected |

## Decision boundary

`0.1.0-alpha.1` cannot be frozen as a portable alpha while the cancellation gate
is blocked. Apache-2.0 and the public companion repository are provisional local
publication metadata, not an upstream RTK contribution decision.

## Dogfooding evidence (2026-07-24)

| Cycle | Work performed | Failure / fallback | Observation |
| --- | --- | --- | --- |
| `rtk-wsl` | Git status and README read using the release binary | None | 14.3 s wall time; treat as a WSL startup outlier, not a performance claim |
| Flowpeek | Git status and README discovery using the same release binary | None | 10.7 s wall time; treat as a WSL startup outlier, not a performance claim |

The package archive contained source, tests, scripts, documentation, and Cargo
metadata only. It did not contain `target/`, workstation configuration, or raw
dogfooding logs.
