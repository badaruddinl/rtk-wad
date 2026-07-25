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

| Workload | Raw median | Native RTK median | WAD native candidate median | WAD auto median | Raw / native / candidate / auto tokens | Auto tokens saved vs raw | Saving | Auto decision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `git status --short --branch` | 127.613 ms | 393.760 ms | 518.466 ms | 266.283 ms | 37 / 37 / 37 / 37 | 0 | 0.0% | Raw: no token reduction and lower latency. |
| `git log --oneline -100` | 126.856 ms | 209.866 ms | 346.381 ms | 309.754 ms | 864 / 864 / 864 / 864 | 0 | 0.0% | Raw: no token reduction and lower latency. |
| Focused `rg` | 71.723 ms | 158.591 ms | 291.880 ms | 138.457 ms | 94 / 94 / 94 / 94 | 0 | 0.0% | Raw: no token reduction and lower latency. |
| Broad `rg` | 69.994 ms | 157.517 ms | 288.386 ms | 293.102 ms | 3,164 / 2,082 / 2,082 / 2,082 | 1,082 | 34.2% | Native RTK: 34.2% token saving clears the 25% threshold, at a measured latency cost. |

The auto result is intentionally separate from the native candidate. The three
zero-saving workloads select raw; the broad search selects native RTK because
its independently measured token saving exceeds the policy threshold. This
separation makes the latency/token trade-off explicit rather than silently
averaging unrelated `rg` forms.

## WSL bridge matrix

| Workload | Raw Windows median | WAD WSL1 median | WAD WSL2 median | Raw / WSL1 / WSL2 tokens | WSL1 saved | WSL2 saved |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `git status --short --branch` | 171.142 ms | 745.905 ms | 3,212.174 ms | 9 / 9 / 9 | 0 (0.0%) | 0 (0.0%) |
| `git log --oneline -100` | 135.484 ms | 600.467 ms | 1,821.236 ms | 864 / 864 / 864 | 0 (0.0%) | 0 (0.0%) |
| Focused `rg` | 80.586 ms | 568.634 ms | 877.106 ms | 94 / 94 / 94 | 0 (0.0%) | 0 (0.0%) |
| Broad `rg` | 65.618 ms | 507.630 ms | 679.022 ms | 3,164 / 2,017 / 2,047 | 1,147 (36.3%) | 1,117 (35.3%) |

For the broad search, WSL1 saved 36.3% and WSL2 saved 35.3% of output tokens.
Both bridge routes remain slower than raw Windows for this warmed corpus; WSL1
is consistently lower-latency than WSL2 here. The v2 bridge protocol records
one artifact per workload with its own WAD state to prevent cross-workload
policy/cache contamination. This is a local observation, not a universal WSL
performance claim.

## Limitations and release interpretation

The machine-readable artifacts and private providers are deliberately ignored
by Git because they contain workstation paths, timing noise, and state. This
record does not claim all RTK command families are performance-covered. P15
proves command-surface inventory parity; P18 requires separate real corpus,
deterministic fixture, or explicit side-effect-contract evidence for every
additional family before publication as covered performance evidence.
