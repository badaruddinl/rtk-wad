# External CLI fixture three-way validation (2026-07-24)

## Scope and method

This validation covers the RTK external adapter families `aws`, `curl`,
`docker`, `gh`, `glab`, `kubectl`, `oc`, `psql`, and `wget`. Each command used
a deterministic Windows executable fixture and a matching WSL1 fixture; no
network service, cloud account, cluster, or database was contacted.

Each family ran one warm-up and five rotated measurements per raw Windows,
stock Windows RTK, and RTK-WAD-auto variant. Coverage required all fifteen
measured commands to exit with `0`, raw execution to receive the caller argv
exactly, and stock RTK plus WAD to produce identical normalized adapter output.
All nine families passed those contracts.

| Command family | Raw median ms | Stock RTK median ms | WAD median ms | Raw / RTK / WAD tokens |
| --- | ---: | ---: | ---: | ---: |
| `aws` | 35.316 | 90.858 | 423.600 | 17 / 38 / 38 |
| `curl` | 36.292 | 108.783 | 440.441 | 15 / 18 / 18 |
| `docker` | 37.387 | 111.204 | 421.921 | 12 / 12 / 12 |
| `gh` | 41.981 | 114.479 | 482.408 | 14 / 34 / 34 |
| `glab` | 35.151 | 101.611 | 455.267 | 15 / 15 / 15 |
| `kubectl` | 40.715 | 98.690 | 434.772 | 14 / 39 / 39 |
| `oc` | 40.531 | 106.304 | 444.915 | 13 / 37 / 37 |
| `psql` | 42.030 | 95.339 | 427.512 | 17 / 17 / 17 |
| `wget` | 39.705 | 107.682 | 451.556 | 15 / 9 / 9 |

## Interpretation

RTK intentionally adds semantic options for some adapters, including JSON or
silent-output flags. Therefore raw and RTK output are not expected to match;
the relevant compatibility contract is exact raw argv and equal RTK behavior
between native Windows RTK and WAD's WSL route.

These fixtures establish structured-argument and process compatibility, not
performance evidence for external services. They must not promote an adaptive
route because live response size, authentication, network latency, and command
side effects differ from a deterministic fixture. The command families remain
on the conservative WSL1 route until separately benchmarked on safe real
read-only corpora.

The machine-readable result remains local and ignored by Git because it
contains workstation timing data.
