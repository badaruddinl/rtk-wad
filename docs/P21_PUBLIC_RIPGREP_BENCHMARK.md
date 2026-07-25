# Public corpus benchmark: ripgrep 14.1.1

This is a supplementary Windows-native evidence row for the P21 branch. It was
run against the public `BurntSushi/ripgrep` tag `14.1.1`, commit
`4649aa9700619f94cf9c66876e9549d83420e16c`, provisioned by the checked-in
public-corpus manifest. It is not a benchmark of the RTK-WAD repository and it
does not promote any unverified command family.

## Inputs

- Stock native RTK: official `v0.43.0` Windows binary, SHA-256
  `a715e989bcebfc208f388cf5adaaa9953cbf1127b081bc09c4ef02e7d7fea39f`.
- RTK command inventory: all 69 top-level commands exactly matched the pinned
  manifest.
- Counter: explicitly installed `tiktoken==0.12.0`, `o200k_base` over combined
  stdout and stderr.
- Protocol: ten warmed, rotating rounds for raw Windows, stock native RTK,
  forced WAD native candidate, and WAD auto after importing the generated local
  policy.
- Search roots: `crates`, `tests`; focused pattern: `RegexBuilder`; broad
  pattern: `fn|struct|impl|use|pub`.

## Results

| Workload | Raw median | Stock native RTK median | WAD native candidate median | WAD auto median | Raw → WAD-auto tokens | Saving | Final route |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `git status --short --branch` | 111.185 ms | 285.750 ms | 408.089 ms | 124.044 ms | 6 → 6 | 0.0% | Raw |
| `git log --oneline -100` | 93.191 ms | 150.335 ms | 273.589 ms | 113.589 ms | 11 → 11 | 0.0% | Raw |
| Focused `rg` | 55.408 ms | 113.195 ms | 208.930 ms | 122.838 ms | 119 → 119 | 0.0% | Raw |
| Broad `rg` | 68.755 ms | 176.197 ms | 289.820 ms | 140.680 ms | 137,700 → 137,700 | 0.0% | Raw |

The result is intentionally negative for filtering: every measured native RTK
output was token-equivalent to raw output on this corpus and command form. WAD
therefore selected raw Windows after policy import. This is evidence that the
dispatcher does not manufacture savings and that prior positive results must be
kept corpus- and command-form-specific.

The machine-readable local artifact retains absolute paths and isolated state
paths, so it is deliberately not committed. The checked-in manifest, provision
script, exact command forms, pins, and table above are sufficient to reproduce
the measurement without exposing workstation paths.
