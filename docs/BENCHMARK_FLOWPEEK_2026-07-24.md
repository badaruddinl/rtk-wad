# Flowpeek core three-way benchmark (2026-07-24)

## Scope and method

This result compares raw Windows commands, stock Windows RTK `0.43.0`, and
`rtk-wad auto` on the clean Flowpeek worktree at commit
`d31c9597238413627cd1d1e52671ccbd82c3422f`. The source worktree was on `E:`;
the WAD metrics ledger and temporary RTK tracker databases were on the Windows
local application-data volume.

Each variant received one warm-up and ten measured runs. Variant order rotated
per round. Token counts use the `o200k_base` tokenizer over combined stdout and
stderr. Latency is wall-clock process time, so it includes launcher overhead.
All sampled commands exited with code `0`.

| Workload | Variant | Median ms | P95 ms | Output tokens | Saving vs raw |
| --- | --- | ---: | ---: | ---: | ---: |
| `git status --short --branch` | Raw Windows | 138.438 | 274.107 | 7 | 0.0% |
|  | Stock RTK | 342.993 | 383.300 | 7 | 0.0% |
|  | RTK-WAD auto | 435.494 | 605.125 | 7 | 0.0% |
| `git log --oneline -100` | Raw Windows | 131.008 | 177.517 | 1,309 | 0.0% |
|  | Stock RTK | 195.129 | 253.562 | 1,309 | 0.0% |
|  | RTK-WAD auto | 303.098 | 430.063 | 1,309 | 0.0% |
| `rg -n graphVersion src test docs` | Raw Windows | 108.258 | 162.451 | 16,466 | 0.0% |
|  | Stock RTK | 190.909 | 217.864 | 5,239 | 68.2% |
|  | RTK-WAD auto | 300.262 | 338.316 | 5,239 | 68.2% |
| broad code `rg` query | Raw Windows | 104.732 | 130.051 | 248,209 | 0.0% |
|  | Stock RTK | 239.560 | 326.223 | 5,869 | 97.6% |
|  | RTK-WAD auto | 437.057 | 559.829 | 5,869 | 97.6% |

## Interpretation

The dispatcher preserved the exact token savings of stock RTK for the two
search workloads. The raw command was fastest for every compact workload in
this sample. WAD added launch and local-accounting overhead over stock native
RTK, so it is not presented as a latency optimization for small output.

The useful decision boundary is output size: use raw execution for mutations
and compact native commands when latency is dominant; use WAD's structured RTK
path when a 68.2% to 97.6% token reduction materially benefits the consuming
agent. The benchmark does not claim that all upstream RTK command families are
covered. The coverage protocol and required deterministic fixtures are listed
in `benchmarks/README.md`.

The machine-readable samples are intentionally local and ignored by Git because
they include workstation paths, timing noise, and the WAD local ledger.
