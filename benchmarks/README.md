# Benchmark protocol

This directory owns reproducible benchmark inputs for the Windows adaptive
dispatcher. Core benchmarks compare four distinct variants:

1. The raw Windows command, with no RTK.
2. Stock native Windows RTK.
3. `rtk-wad --route native-rtk`, including dispatcher and ledger overhead for
   the native candidate.
4. `rtk-wad` in auto mode after the generated policy is imported into an
   isolated state.

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
integration into a synthetic success. For publishable evidence, use the pinned,
public projects in [`public-corpora.json`](public-corpora.json), not the WAD
repository or a private workstation project. Provision them outside the WAD
worktree; the script pins the tag and commit, reuses only an exact existing
clone, and never overwrites a corpus:

```powershell
.\scripts\provision-public-benchmark-corpus.ps1 -Corpus ripgrep-14.1.1
```

For a read-only workload that needs only selected repository files, use an
explicit sparse checkout. It is still pinned to the manifest's exact Git commit
and origin, but avoids downloading unrelated blobs. For example, the TypeScript
`npm run` benchmark needs only its root package manifest:

```powershell
.\scripts\provision-public-benchmark-corpus.ps1 `
  -Corpus typescript-5.9.3 `
  -SparsePath package.json
```

Sparse mode is opt-in. The default corpus provisioner continues to create a
complete checkout for source-search and toolchain workloads.

The optional `tiktoken` Python environment is required only by a benchmark
runner. Install it explicitly with `scripts/install.ps1 -InstallTokenizer`.
Command families are classified before a run as follows:

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

The runner requires the explicitly installed WAD benchmark Python environment
with `tiktoken` and does not silently substitute a different tokenizer. It
intentionally reports output-equivalence evidence rather
than asserting byte equality: RTK is expected to reduce output.

The ripgrep corpus is discovered from existing `src`, `tests`, `test`, and
`docs` directories. A missing conventional directory is never passed to a
benchmark command; if none exists the run fails before measurement and records
no evidence.

```powershell
node .\benchmarks\run-core-three-way.mjs `
  --repo E:\luthfi\project\flowpeek `
  --native-rtk C:\tools\rtk.exe `
  --wad C:\tools\rtk-wad.exe `
  --python C:\path\to\python.exe `
  --preflight .\.flowpeek\cache\p18-benchmark-preflight.json `
  --output .\benchmarks\results\flowpeek.json `
  --install-policy
```

For a public corpus with different source roots or symbols, make those choices
explicit in the artifact rather than relying on WAD's directory conventions:

```powershell
node .\benchmarks\run-core-three-way.mjs `
  --repo $env:LOCALAPPDATA\rtk-wad\benchmark-corpora\ripgrep-14.1.1 `
  --search-roots crates,tests `
  --focused-pattern RegexBuilder `
  --broad-pattern 'fn|struct|impl|use|pub' `
  # ...the same native RTK, WAD, Python, preflight, and output options
```

Set `RTK_WAD_BENCH_GIT` or `RTK_WAD_BENCH_RG` only when the corresponding raw
Windows executable is not discoverable on `PATH`. Benchmark output is an
English, machine-readable artifact; the release report must include both wins
and losses.

The runner always imports the generated policy into an isolated benchmark state
to measure the final auto decision. `--install-policy` separately imports the
same validated policy into the caller's normal WAD state. It never uses fixture
output as performance evidence.

The core runner rejects every non-zero or signalled warm-up/sample and requires
a P18 preflight that contains the exact native RTK path. It emits a P16 policy
schema with the current manifest version and opaque local context signature;
an outdated or mismatched policy is not written as importable evidence.

## WSL bridge runner

`run-wsl-bridge-core.mjs` measures the same safe Git and ripgrep corpus through
explicit WAD WSL1 and WSL2 routes, alongside raw Windows execution. Both WSL
providers must appear in the P18 preflight with exact command-manifest coverage.
It never treats WSL output as a replacement for the native Windows RTK row.

```powershell
node .\benchmarks\run-wsl-bridge-core.mjs `
  --repo E:\luthfi\project\rtk-wsl `
  --wad C:\tools\rtk-wad.exe `
  --python C:\tools\rtk-wad\tokenizer\Scripts\python.exe `
  --preflight .\.flowpeek\cache\p18-benchmark-preflight.json `
  --wsl1-distro Ubuntu-RTK-WSL1 `
  --wsl1-rtk /home/rtk/.rtk-wad-benchmark/v0.43.0/rtk `
  --wsl2-distro Ubuntu `
  --wsl2-rtk /home/badaruddinl/.local/bin/rtk `
  --output .\benchmarks\results\wsl-bridge-core.json
```

Use `run-toolchain-three-way.mjs --tool cargo --policy-key cargo:check --
check` for Cargo evidence. It requires a P18 preflight and a private WAD state
per output artifact. Set `RTK_WAD_BENCH_TOOL` to an explicit `cargo.exe` when
the selected toolchain is not already on `PATH`. The retired
`run-cargo-three-way.mjs` schema-v1 runner is intentionally absent so it cannot
generate an importable-looking but invalid policy artifact.

## Node package-manager runner

`run-npm-run-list-three-way.mjs` measures the read-only `npm run` script-list
operation on a real Windows worktree. It requires P18 preflight for the exact
stock native RTK, records raw Windows, the explicit WAD native candidate, and
WAD auto mode after importing a v2 context-bound policy into state unique to
the artifact. It records hashes, exit codes, latency, and `o200k_base` token
counts for all variants. `npm run <script>` is intentionally out of scope
because scripts may mutate source, dependencies, or external state.

```powershell
node .\benchmarks\run-npm-run-list-three-way.mjs `
  --repo E:\luthfi\project\flowpeek `
  --native-rtk C:\tools\rtk.exe `
  --wad C:\tools\rtk-wad.exe `
  --python C:\path\to\python.exe `
  --preflight .\.flowpeek\cache\p18-benchmark-preflight.json `
  --output .\benchmarks\results\flowpeek-npm-run-list.json
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
  --preflight .\.flowpeek\cache\p18-benchmark-preflight.json `
  --output .\benchmarks\results\go-test.json `
  -- test ./...
```

Pass `--policy-key` only for an exactly verified command form with a stock RTK
comparison. The runner requires the P18 preflight to name that exact native
RTK, obtains a v2 local policy context, measures an explicit WAD `native-rtk`
candidate, imports only that context-bound policy into a state directory unique
to the artifact, and finally measures WAD auto mode. Static raw fallbacks
remain compatibility defaults until repeated real-project evidence justifies
such a narrow policy rule. Each command has a 60-second default process-tree
deadline; pass `--timeout-ms` to record a stricter or looser operational limit.
A timeout is a failed measurement and never becomes release evidence.

`--skip-warmup` is permitted only after one successful raw and one successful
WAD warm-up have been run separately and recorded in the release note. It
exists for slow workloads whose complete three-way execution would exceed the
host command deadline; it never lowers the five measured-round requirement.
The runner isolates only WAD's ledger via `RTK_WAD_STATE_DIR`; it intentionally
does not replace `LOCALAPPDATA`, so raw Windows toolchains retain their normal
per-user caches.

The default comparison is byte-exact. The only current exceptions are
`--normalizer cargo-check-duration`, `--normalizer dart-format-duration`, and
`--normalizer flutter-analysis-duration`: they replace only the respective
non-semantic elapsed-time field before semantic comparison while retaining the
unmodified output hashes. Do not add a normalizer merely to make a benchmark
pass; it must remove only a documented, non-semantic volatile field.

## External-service fixture runner

The `fixtures` directory supplies executable doubles for AWS, curl, Docker,
GitHub CLI, GitLab CLI, Kubernetes, OpenShift, PostgreSQL, and wget. Install the
Windows and WSL1 fixture directories, then run `run-fixture-three-way.mjs` with
both paths, the selected WSL1 distro/RTK path, and the P18 preflight. WAD is
forced to WSL1 and receives the Linux fixture directory through
`RTK_WSL_EXTRA_PATH`; stock RTK receives the Windows fixture directory through
its child PATH. The runner rejects coverage when a variant exits unsuccessfully,
raw execution does not receive exact argv, or stock RTK and WSL1 WAD differ in
normalized adapter output. It never emits a route policy: fixture results prove
compatibility only and cannot promote adaptive execution.
