# Public corpus benchmark: TypeScript 5.9.3 `npm run`

This supplementary Windows-native evidence row measures the read-only `npm run`
script-list operation against the public Microsoft TypeScript tag `v5.9.3`,
commit `c63de15a992d37f0d6cec03ac7631872838602cb`. It is not a benchmark of the
RTK-WAD repository and it does not run project scripts or install Node packages.

## Corpus and inputs

- Corpus: a blob-filtered sparse checkout containing only the pinned root
  `package.json`, provisioned with `provision-public-benchmark-corpus.ps1
  -Corpus typescript-5.9.3 -SparsePath package.json`. This is sufficient for
  `npm run`, avoids project dependency installation, and retains the Git origin
  and exact commit verification.
- Stock native RTK: official `v0.43.0` Windows binary, SHA-256
  `a715e989bcebfc208f388cf5adaaa9953cbf1127b081bc09c4ef02e7d7fea39f`.
- Counter: explicitly installed `tiktoken==0.12.0`, `o200k_base` over combined
  stdout and stderr.
- Protocol: one successful warm-up followed by five rotating rounds for raw
  Windows, stock native RTK, forced WAD native candidate, and WAD auto after a
  context-bound policy import.

## Results

| Variant | Median | P95 | Tokens | Saving versus raw | Output bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raw Windows | 1,622.328 ms | 2,182.231 ms | 147 | 0.0% | 517 |
| Stock native RTK | 1,849.089 ms | 2,804.244 ms | 147 | 0.0% | 517 |
| Forced WAD native candidate | 2,200.362 ms | 2,369.263 ms | 147 | 0.0% | 517 |
| WAD auto after policy | 962.514 ms | 1,003.079 ms | 147 | 0.0% | 517 |

All recorded commands exited successfully. Raw and WAD-auto emitted the same
single output hash, so the coverage row is valid. The policy recorded zero
token saving and slower native-candidate latency; `rtk-wad --explain-route npm
run` subsequently reported `route=raw` with the lower-latency raw reason.

The faster auto timings are not treated as a performance claim: npm's Windows
launcher and process caches are host-sensitive. The decisive facts are output
equivalence, zero token saving, and the explicit raw routing decision. The
machine-readable artifact remains local because it contains absolute paths and
isolated state locations.
