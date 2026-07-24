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

## External-service fixture runner

The `fixtures` directory supplies executable doubles for AWS, curl, Docker,
GitHub CLI, GitLab CLI, Kubernetes, OpenShift, PostgreSQL, and wget. Install the
Windows and WSL fixture directories, then run `run-fixture-three-way.mjs` with
both paths. WAD receives the Linux fixture directory through
`RTK_WSL_EXTRA_PATH`; stock RTK receives the Windows fixture directory through
its child PATH. The fixture runner records the same three-way evidence without
contacting a network service.
