# NPM run-list three-way benchmark (2026-07-24)

## Scope and method

This result measures the exact read-only `npm run` form on the local Flowpeek
worktree. It does not measure `npm run <script>` because a project script can
change source files, dependencies, or external state. The source worktree was
on `E:`; WAD's temporary tracker and ledger were redirected to a local Windows
application-data directory.

The benchmark ran one warm-up followed by five rotated measurements per
variant. Wall-clock latency includes process-launch overhead. Token counts use
`o200k_base` over combined stdout and stderr. All fifteen measured commands
exited with code `0`, and each variant produced one stable, identical output
hash.

| Variant | Median ms | P95 ms | Output tokens | Saving vs raw |
| --- | ---: | ---: | ---: | ---: |
| Raw Windows `npm.cmd run` | 585.190 | 607.912 | 728 | 0.0% |
| Stock Windows RTK `npm run` | 617.966 | 765.079 | 728 | 0.0% |
| RTK-WAD auto | 635.791 | 886.289 | 728 | 0.0% |

## Decision

The generated local policy selects raw Windows execution for the exact
`npm run` listing operation because it has no token-saving benefit and is
faster end-to-end. WAD keeps `npm run <script>` on the static raw route; it is
not eligible for native RTK promotion. This result is local evidence for this
project and machine, not a claim that every NPM project has the same latency.

The machine-readable benchmark and installed policy remain local and ignored by
Git because they include workstation paths and timing noise.
