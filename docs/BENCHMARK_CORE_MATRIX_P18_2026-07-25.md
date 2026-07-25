# P18 core benchmark matrix (2026-07-25)

## Scope

This local, reproducible record measures the `rtk-wsl` worktree on Windows
using RTK `0.43.0`. The provider preflight verified the identical 69-command
surface for the isolated native Windows binary, the dedicated WSL1 binary, and
the selected Ubuntu WSL2 binary. It is evidence for these exact commands,
machine, provider paths, and warmed corpus only.

Each row used five rotating warm measurements. Every recorded invocation exited
with code `0` and no signal. Latency is end-to-end process wall time. Token
counts use `o200k_base` through the pinned private dependency
`tiktoken==0.12.0` over combined standard output and standard error.

The native matrix compares raw Windows, stock Windows RTK, an explicit WAD
native candidate, and WAD auto mode after importing the generated policy into a
private benchmark state. The bridge matrix separately compares raw Windows
with forced WAD WSL1 and WSL2 routes. It does not use a WSL provider as a stand
in for a Windows-native RTK measurement.

## Native Windows matrix

| Workload | Raw median | Native RTK median | WAD native candidate median | WAD auto median | Raw / native / candidate / auto tokens | Auto decision |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `git status --short --branch` | 104.755 ms | 198.759 ms | 326.696 ms | 125.787 ms | 55 / 55 / 55 / 55 | Raw: no token reduction and lower latency. |
| `git log --oneline -100` | 97.718 ms | 169.236 ms | 267.949 ms | 183.169 ms | 754 / 754 / 754 / 754 | Raw: no token reduction and lower latency. |
| Focused `rg` | 62.378 ms | 141.915 ms | 259.572 ms | 118.727 ms | 94 / 94 / 94 / 94 | Raw: no token reduction and lower latency. |
| Broad `rg` | 59.919 ms | 124.474 ms | 249.994 ms | 116.439 ms | 2,963 / 1,888 / 1,888 / 2,963 | Raw: 36.3% native saving exists, but the combined `rg` evidence is 18.15%, below the 25% policy threshold. |

The auto result is intentionally separate from the native candidate. It proves
the dispatcher does not misrepresent candidate token reduction as an automatic
choice when the context-bound policy selects raw execution.

## WSL bridge matrix

| Workload | Raw Windows median | WAD WSL1 median | WAD WSL2 median | Raw / WSL1 / WSL2 tokens |
| --- | ---: | ---: | ---: | ---: |
| `git status --short --branch` | 126.316 ms | 606.762 ms | 2,325.243 ms | 66 / 66 / 66 |
| `git log --oneline -100` | 99.666 ms | 367.472 ms | 1,254.799 ms | 754 / 754 / 754 |
| Focused `rg` | 46.438 ms | 415.204 ms | 640.780 ms | 94 / 94 / 94 |
| Broad `rg` | 53.886 ms | 422.414 ms | 649.428 ms | 2,963 / 1,826 / 1,856 |

For the broad search, WSL1 saved 38.4% and WSL2 saved 37.4% of output tokens.
Both bridge routes remain slower than raw Windows for this warmed corpus; WSL1
is consistently lower-latency than WSL2 here. This is a local observation, not
a universal WSL performance claim.

## Limitations and release interpretation

The machine-readable artifacts and private providers are deliberately ignored
by Git because they contain workstation paths, timing noise, and state. This
record does not claim all RTK command families are performance-covered. P15
proves command-surface inventory parity; P18 requires separate real corpus,
deterministic fixture, or explicit side-effect-contract evidence for every
additional family before publication as covered performance evidence.
