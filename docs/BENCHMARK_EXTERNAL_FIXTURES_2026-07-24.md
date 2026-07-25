# External CLI fixture compatibility validation (P18, 2026-07-25)

## Scope and method

This validation covers the RTK external adapter families `aws`, `curl`,
`docker`, `gh`, `glab`, `kubectl`, `oc`, `psql`, and `wget`. Each command used
a deterministic Windows executable fixture and a matching WSL1 fixture; no
network service, cloud account, cluster, or database was contacted.

Each family ran one warm-up and five rotated measurements per raw Windows,
stock Windows RTK, and forced RTK-WAD WSL1 variant. The P18 preflight verified
the exact Windows and WSL1 RTK paths before each batch. Coverage required all
fifteen measured commands to exit with `0`, raw execution to receive the caller
argv exactly, and stock RTK plus forced WSL1 WAD to produce identical normalized
adapter output. All nine families passed those contracts.

| Command family | Raw median ms | Stock RTK median ms | WAD median ms | Raw / RTK / WAD tokens |
| --- | ---: | ---: | ---: | ---: |
| `aws` | 51.444 | 134.556 | 485.952 | 17 / 38 / 38 |
| `curl` | 71.349 | 184.463 | 573.625 | 15 / 18 / 18 |
| `docker` | 60.034 | 138.537 | 531.954 | 12 / 12 / 12 |
| `gh` | 81.848 | 189.430 | 707.266 | 14 / 34 / 34 |
| `glab` | 101.225 | 132.312 | 661.300 | 15 / 15 / 15 |
| `kubectl` | 51.047 | 121.912 | 461.686 | 14 / 39 / 39 |
| `oc` | 49.576 | 124.742 | 402.012 | 13 / 37 / 37 |
| `psql` | 50.310 | 118.544 | 461.843 | 17 / 17 / 17 |
| `wget` | 50.866 | 134.673 | 478.803 | 15 / 9 / 9 |

## Interpretation

RTK intentionally adds semantic options for some adapters, including JSON or
silent-output flags. Therefore raw and RTK output are not expected to match;
the relevant compatibility contract is exact raw argv and equal RTK behavior
between native Windows RTK and WAD's WSL route.

These fixtures establish structured-argument and process compatibility, not
adaptive performance evidence for external services. Their v2 artifact sets
`adaptive_policy_eligible` to `false`: live response size, authentication,
network latency, and command side effects differ from a deterministic fixture.
The command families remain on the conservative WSL1 route until separately
benchmarked on safe real read-only corpora.

The machine-readable result remains local and ignored by Git because it
contains workstation timing data.
