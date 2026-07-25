# NPM run-list four-way benchmark (P18, 2026-07-25)

## Scope and method

This result measures the exact read-only `npm run` form on the local `kas-new`
worktree. It does not measure `npm run <script>` because a project script can
change source files, dependencies, or external state. The source worktree was
on `E:`. The P18 preflight verified the exact stock Windows RTK provider, while
the WAD policy state was isolated per artifact.

The benchmark ran one warm-up followed by five rotated baseline measurements,
then five WAD auto measurements after importing context-bound v2 evidence.
Wall-clock latency includes process-launch overhead. Token counts use
`o200k_base` through `tiktoken==0.12.0` over combined stdout and stderr. All
twenty measured commands exited with code `0`.

| Variant | Median ms | P95 ms | Output tokens | Saving vs raw |
| --- | ---: | ---: | ---: | ---: |
| Raw Windows `npm.cmd run` | 697.262 | 1,757.321 | 167 | 0.0% |
| Stock Windows RTK `npm run` | 767.649 | 3,448.344 | 167 | 0.0% |
| RTK-WAD native candidate | 1,072.503 | 4,017.465 | 167 | 0.0% |
| RTK-WAD auto after policy | 988.015 | 1,063.732 | 167 | 0.0% |

## Decision

The generated local policy selects raw Windows execution for the exact `npm
run` listing operation because it has no token-saving benefit and is faster
end-to-end. WAD keeps `npm run <script>` on the static raw route; it is not
eligible for native RTK promotion. This result is local evidence for this
project and machine, not a claim that every NPM project has the same latency.

The machine-readable benchmark and installed policy remain local and ignored by
Git because they include workstation paths and timing noise.
