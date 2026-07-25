# Three-way benchmark protocol

This directory owns reproducible benchmark inputs for the Windows adaptive
dispatcher. Every published result compares exactly three variants:

1. The raw Windows command, with no RTK.
2. Stock native Windows RTK.
3. `rtk-wad` in `auto` mode.

`command-manifest.json` is the authoritative v0.43.0 top-level command inventory
for this repository. Its conservative WSL1 classification is intentional: a
command moves to native RTK only after its structured-argv and no-replay
contracts are proven on Windows.

Before collecting any P18 performance row, run the provider and manifest
preflight. It records whether the actual Windows, WSL1, and WSL2 RTK binaries
match the complete 69-command inventory; it never installs or substitutes a
missing provider:

```powershell
.\scripts\audit-provider-baseline.ps1 `
  -OutputPath .\.flowpeek\cache\p18-benchmark-preflight.json
.\tests\benchmark-preflight-contract.ps1
```

See [`BENCHMARK_MATRIX_P18.md`](../docs/BENCHMARK_MATRIX_P18.md). A missing
native Windows RTK or WSL RTK is a blocked evidence row, not permission to use
another backend as a stand-in.

```powershell
.\benchmarks\verify-command-manifest.ps1 -NativeRtk C:\tools\rtk.exe
```

The benchmark runner uses a real local corpus and never turns a failed external
integration into a synthetic success. Command families are classified before a
run as follows:

| Coverage tier | Command families | Evidence requirement |
| --- | --- | --- |
| Real corpus | `git`, `rg`, `grep`, `find`, `ls`, `tree`, `read`, `diff` | Windows repository with stable fixture files. |
| Deterministic fixture | `docker`, `kubectl`, `oc`, `aws`, `gh`, `glab`, `psql`, `curl`, `wget` | Executable fixture that records received argv and returns fixed output. |
| Toolchain corpus | `cargo`, `npm`, `pnpm`, `npx`, `jest`, `vitest`, `tsc`, `lint`, `prettier`, `format`, `pytest`, `mypy`, `ruff`, `go`, `dotnet`, `gradlew`, `mvn`, `rake`, `rubocop`, `rspec`, `pip` | Pinned sample project and lockfile. |
| RTK internal | `gain`, `discover`, `learn`, `init`, `config`, `rewrite`, `hook`, `telemetry`, `trust`, `untrust`, `verify`, `pipe`, `run`, `proxy` | Explicit side-effect policy and process-contract test. |

No row may be labelled *covered* until its raw command, stock RTK command, and
WAD command all have recorded exit status, stdout/stderr hash, latency, and
`o200k_base` token count. External services must use deterministic fixtures in
CI; live services can be recorded as supplementary evidence only.

## Core runner

`run-core-three-way.mjs` executes the four high-signal real-corpus workloads:
Git status, Git log, a focused ripgrep query, and a broad ripgrep query. It
performs one warm-up and ten rotating measured rounds per variant. The resulting
JSON records each sample plus median and p95 latency, output bytes, exit codes,
SHA-256 output hashes, and exact `o200k_base` counts.

The runner requires Python with `tiktoken` and does not silently substitute a
different tokenizer. It intentionally reports output-equivalence evidence rather
than asserting byte equality: RTK is expected to reduce output.

```powershell
node .\benchmarks\run-core-three-way.mjs `
  --repo E:\luthfi\project\flowpeek `
  --native-rtk C:\tools\rtk.exe `
  --wad C:\tools\rtk-wad.exe `
  --python C:\path\to\python.exe `
  --output .\benchmarks\results\flowpeek.json `
  --install-policy
```

Set `RTK_WAD_BENCH_GIT` or `RTK_WAD_BENCH_RG` only when the corresponding raw
Windows executable is not discoverable on `PATH`. Benchmark output is an
English, machine-readable artifact; the release report must include both wins
and losses.

`--install-policy` imports the policy generated from that same real-corpus run
into the local WAD data directory. It never uses fixture output as performance
evidence.

`run-cargo-three-way.mjs` measures `cargo check` on a real Windows worktree. It
requires `--target-dir` on NTFS so build cache churn is isolated from a source
worktree that may reside on exFAT or another non-NTFS volume. Set
`RTK_WAD_BENCH_CARGO` to an explicit `cargo.exe` when the selected toolchain is
not already on `PATH`.

## Node package-manager runner

`run-npm-run-list-three-way.mjs` measures the read-only `npm run` script-list
operation on a real Windows worktree. It compares `npm.cmd`, stock native RTK,
and WAD auto mode with rotating warm and measured rounds. It records hashes,
exit codes, latency, and `o200k_base` token counts for all variants. A route
policy is emitted only when every measured command exits successfully; it is
limited to the exact `npm run` form. `npm run <script>` is intentionally out of
scope because scripts may mutate source, dependencies, or external state.

```powershell
node .\benchmarks\run-npm-run-list-three-way.mjs `
  --repo E:\luthfi\project\flowpeek `
  --native-rtk C:\tools\rtk.exe `
  --wad C:\tools\rtk-wad.exe `
  --python C:\path\to\python.exe `
  --output .\benchmarks\results\flowpeek-npm-run-list.json `
  --install-policy
```

Set `RTK_WAD_BENCH_NPM` to an explicit `npm.cmd` only when it is not available
on `PATH`.

## Provider-cache profile

`run-provider-cache-profile.ps1` measures the cold and warm automatic Go
provider path against direct raw Go. It uses a temporary private WAD state
directory and records JSON only; it does not generate a route policy or invoke
an installer. See
[`ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md`](../docs/ADAPTIVE_CACHE_BENCHMARK_P7_2026-07-25.md)
for the current-head result and interpretation.

## Generic toolchain runner

`run-toolchain-three-way.mjs` records the same latency, exit, output-hash, and
`o200k_base` evidence for a pinned toolchain workload. It asserts that raw
Windows and WAD output are identical. Use its normal three-way form only for a
top-level stock RTK command, such as `go`. Use `--without-native` for WAD-owned
Windows shims such as `dart` and `flutter`: stock RTK v0.43.0 has no equivalent
top-level command, so treating a rejection as a benchmark result would be
misleading.

```powershell
node .\benchmarks\run-toolchain-three-way.mjs `
  --tool go `
  --repo E:\luthfi\project\go-practice `
  --native-rtk C:\tools\rtk.exe `
  --wad C:\tools\rtk-wad.exe `
  --python C:\path\to\python.exe `
  --output .\benchmarks\results\go-test.json `
  -- test ./...
```

Pass `--policy-key` only for an exactly verified command form with a stock RTK
comparison; the runner then emits a one-key route-policy artifact. Static raw
fallbacks remain compatibility defaults until repeated real-project evidence
justifies such a narrow policy rule. A policy run adds an explicit WAD
`native-rtk` candidate sample, so its latency includes dispatcher and local
accounting overhead rather than reusing the stock RTK latency. Each command has
a 60-second default process-tree deadline; pass
`--timeout-ms` to record a stricter or looser operational limit. A timeout is a
failed measurement and never becomes release evidence.

`--skip-warmup` is permitted only after one successful raw and one successful
WAD warm-up have been run separately and recorded in the release note. It
exists for slow workloads whose complete three-way execution would exceed the
host command deadline; it never lowers the five measured-round requirement.
The runner isolates only WAD's ledger via `RTK_WAD_STATE_DIR`; it intentionally
does not replace `LOCALAPPDATA`, so raw Windows toolchains retain their normal
per-user caches.

The default comparison is byte-exact. The only current exceptions are
`--normalizer dart-format-duration` and `--normalizer flutter-analysis-duration`:
they replace the respective elapsed-time field before semantic comparison while
retaining the unmodified output hashes. Do not add a normalizer merely to make
a benchmark pass; it must remove only a documented, non-semantic volatile
field.

## External-service fixture runner

The `fixtures` directory supplies executable doubles for AWS, curl, Docker,
GitHub CLI, GitLab CLI, Kubernetes, OpenShift, PostgreSQL, and wget. Install the
Windows and WSL fixture directories, then run `run-fixture-three-way.mjs` with
both paths. WAD receives the Linux fixture directory through
`RTK_WSL_EXTRA_PATH`; stock RTK receives the Windows fixture directory through
its child PATH. The fixture runner rotates variants and rejects coverage when a
variant exits unsuccessfully, raw execution does not receive the exact caller
argv, or stock RTK and WAD differ in normalized adapter output. RTK may add its
documented semantic options (such as JSON output), so raw and RTK output are not
required to be byte-identical. It records the same three-way evidence without
contacting a network service.
