# P7 — Dispatcher Foundation

This milestone does not rename the product and does not provision software.
Its purpose is to prove that RTK is an optional output adapter, rather than the
mechanism that discovers or starts a command.

## Frozen v0.3.0 baseline

- Release tag: `v0.3.0` (`f2351aeaac02243d105d607092ed7f46eafc72bc`).
- The starting `master` commit adds only `.github/FUNDING.yml` after that tag;
  the executable source, packaging scripts, installer, and release documents
  are unchanged.
- Baseline local gate: `cargo test --bin rtk-wad -- --test-threads=1` passed
  before the refactor (42 tests).
- The fixed performance comparator is the v0.3.0 P18 artifact
  [`benchmarks/evidence/p18-comparison-summary.json`](../benchmarks/evidence/p18-comparison-summary.json)
  (Git blob `2162d175a1199154328bf5f227bf775afac9f7bb`) together with its
  source record
  [`docs/BENCHMARK_CORE_MATRIX_P18_2026-07-25.md`](BENCHMARK_CORE_MATRIX_P18_2026-07-25.md)
  (Git blob `46e6240521ecc7b9b46100f57bc4bb7ce1836274`). Both paths resolve
  from `v0.3.0`, and the JSON blob remains identical at the current HEAD.
  New P7 measurements are supplementary: they must name their binary revision
  and never replace this comparator.
- `scripts/verify-release.ps1` remains the strict P20 *benchmark* gate: its
  explicit native RTK, WSL1 RTK, and WSL2 RTK inputs are evidence requirements
  for token/latency claims, not prerequisites for raw dispatcher operation.
  It is therefore not run on this machine. No provider was installed or
  modified to make benchmark evidence pass.
- The available Windows/WSL process-contract run passed 28 of 28 tests.
  Tests execute a verified RTK/WSL1 candidate when it exists; otherwise they
  assert that resolution stays successful through the verified raw candidate.
  This machine exposes Ubuntu WSL2 without RTK and no WSL1 fixture. The suite
  covers argv quoting, stdin, CWD mapping, exit codes, Ctrl+C, locks, raw
  provider execution, and optional-provider absence.

The following remain behavioural compatibility gates: literal argv forwarding,
stdin/stdout/stderr, child exit codes, WSL CWD mapping, and cancellation. Shell
grammar (`cd`, aliases, pipes, redirection) remains owned by PowerShell, CMD,
or Git Bash and is not parsed as an executable command by RTK-WAD.

## Dispatcher contract

The internal `dispatcher` module defines these boundaries:

1. `CommandSpec`: executable, `OsString` argv, CWD, environment overlay, and
   interactive intent.
2. `RouteCandidate`: Windows, WSL1, or WSL2 process location.
3. `ExecutionPlan`: a chosen candidate, an output adapter, and explainable
   decisions.
4. `OutputAdapter`: `raw` starts the discovered binary directly; `rtk` is an
   optional adapter around it.
5. `Provisioner`: a separate approval-gated contract. It is not invoked by
   discovery, route selection, or normal execution in P7.

No user argv is converted to a shell string. The fixed WSL launcher remains
the only shell fragment, and it receives a verified executable plus structured
arguments.

The provider executor now consumes `ExecutionPlan` directly. Route transport
is selected from `RouteCandidate` and output adaptation from `OutputAdapter`;
it does not branch again on the legacy provider label after planning. The
legacy `windows_raw` / `windows_rtk` / `wsl_raw` / `wsl_rtk` labels remain in
diagnostics for v0.3 compatibility, but are not the execution source of truth.

A missing provider is an unavailable candidate, not a resolver error. Auto
selection excludes it and continues with the next compatible raw or RTK plan.
If the chosen executable disappears before a child process starts, the plan
executor retries the next verified candidate only for that `NotFound` start
failure. It never retries a process that already started or hides its exit
code. An explicitly requested route remains an explicit user constraint.

Dispatcher-owned commands are resolved before provider discovery:
`rtk-wad --version`, `rtk-wad version`, and `rtk-wad -V` return the package
version without requiring a Windows executable, WSL distribution, or RTK.

## Implementation boundaries and hot path

The executable keeps configuration, bridge decoding, and drive-path mapping in
separate reusable modules (`config`, `bridge`, and `paths`). `main.rs` owns the
application composition while those modules own their respective validation
rules. This is an incremental extraction: provider discovery, route policy,
and process execution retain their existing contracts while further module
boundaries are introduced behind the same tests.

Device process construction is split under `adapters/`: `windows` owns native
raw/RTK process creation and structured `CommandSpec` application, while
`wsl1` and `wsl2` expose separate transports over one shared safe `wsl.exe`
builder. The shared builder contains only invariant Windows process-group
setup; route-specific lock, cancellation, and wait policies remain distinct.

The provider fast path probes Windows first when the selected route does not
need a WSL inventory. It returns immediately for a verified Windows candidate.
If that candidate is absent, discovery expands once to the complete WSL
inventory; routes already targeting WSL request that complete inventory from
the start. This avoids duplicate full discovery without changing WSL-only
selection. A local warm-process check measured `rtk-wad --version` at
approximately 46â€“62 ms; a post-build cold start is materially slower and is
reported as process-start overhead rather than resolver latency.

## Go vertical slice

`rtk-wad go version` now accepts a verified WSL Go candidate whether it has
RTK installed or not. If the Go binary exists only in a mapped WSL project,
the selected WSL executable is launched as `raw`; this is the expected path
for a PowerShell, CMD, or Git Bash invocation on Windows.

On the current machine, both `rtk-wad go version` and the same command with
`RTK_WAD_OUTPUT_ADAPTER=raw` returned the native Go version and exit code 0.
The process contract also proves the required Windows-to-WSL case live: it
hides Windows Go from `PATH`, exposes a temporary Go fixture only through
`RTK_WSL_EXTRA_PATH`, runs `rtk-wad go version`, checks the WSL-only result,
and removes the fixture.

That fixture is also invoked through real PowerShell, CMD, and Git Bash
processes. Every shell must return the WSL-only version marker and preserve a
single literal argument containing a space, Unicode, `$`, `&`, and a
backslash. The CMD case uses an ASCII batch wrapper with a UTF-16 environment
value, so its own parsing rules—not a pre-escaped Rust command line—handle
the literal argument. The Git Bash case explicitly disables MSYS path
conversion for the native child so the Linux-valued `RTK_WSL_EXTRA_PATH`
remains a Linux path; this captures the shell boundary rather than masking it
with a Windows path rewrite.

A second fixture starts from a Windows CWD containing spaces and Unicode, then
asserts `--explain-route` reports `wsl2` with the `raw` adapter, `which` sees
the WSL binary from a cache hit, the mapped Linux CWD and literal argv reach
the child unchanged, and child exit code `42` reaches the Windows caller. It
then changes the temporary binary and proves that identity revalidation turns
the next lookup into a cache miss. The same fixture runs `doctor go` and
asserts its inspected WSL2 distribution, binary location, usable candidate,
mapped project directory, and recommendation diagnosis.

### Current Go matrix evidence

| Requested case | Evidence in this branch | Status |
| --- | --- | --- |
| Windows project, Windows Go | Local `rtk-wad go version` and `provider exec go -- version` | Live on this host |
| Windows project, WSL2-only Go | Temporary `/tmp` Go fixture; route, raw adapter, CWD, argv, cache, and exit `42` | Live process contract |
| Windows project, WSL1-only Go | No WSL1 candidate is installed; resolver retains verified raw candidates and reports the absence | Optional provider unavailable, no dispatch error |
| WSL project, same distro | `rtk-wad-wsl.sh` process contract from `/tmp` | Live on Ubuntu WSL2 |
| WSL project, other WSL distro | `docker-desktop` source → Ubuntu Go-only fixture through a Windows-mounted project path | One live WSL2 process contract |
| WSL project, Windows Go | Ubuntu shim from Windows-mounted checkout selects Windows Go raw; child and `--explain-route` are checked | One live WSL2 process contract |
| Go unavailable | Read-only `doctor`/setup diagnosis; no install is applied | Covered without provisioning |

### WSL-origin shim

`scripts/rtk-wad-wsl.sh` is the explicit adapter for invoking the Windows
dispatcher from a WSL shell. WSL interop does not pass arbitrary Linux
environment variables to `.exe` processes, so the versioned bridge payload
sends the WSL distro, physical CWD, its drive-qualified `wslpath -w` Windows
mapping when one exists, optional WSL search path, output-adapter preference,
and NUL-delimited UTF-8 argv as one base64 argument. UNC mappings for native
Linux paths are intentionally omitted. The Windows binary decodes that payload
before resolution; no user argv is inserted into a CMD, PowerShell, or shell
command string.

```sh
export RTK_WAD_WINDOWS_EXE=/mnt/c/tools/rtk-wad.exe
export RTK_WSL_EXTRA_PATH=/home/me/.local/bin
sh /mnt/c/tools/rtk-wad-wsl.sh go version
```

One process contract starts this shim from `/tmp` in Ubuntu, points it at a Go
binary available only in the same WSL distribution, and verifies that the
dispatcher selects WSL2, keeps CWD `/tmp`, and preserves a literal argument
containing spaces, Unicode, `$`, `&`, and a backslash. A second contract starts
in the `docker-desktop` WSL2 distro from a Windows-mounted checkout, discovers
Go only in Ubuntu, translates the source CWD through its verified Windows path,
and verifies Ubuntu receives `/mnt/d/.../rtk-wad` plus the literal argv. It is
evidence for this controlled two-distro WSL2 route, not a claim that arbitrary
Linux-native paths can cross distributions.

`CommandSpec` overlays CWD and environment on Windows plan execution. WSL
plans send validated POSIX `KEY=VALUE` overlays as argv before the executable;
the fixed launcher passes them to `env -i` without shell evaluation. Interactive
TTY remains inherited from the calling console, as in the v0.3 process
contract.

The normal `rtk-wad go ...` path now carries that same plan to execution, not
a second route/config/adapter decision. A requested `rtk` adapter fails when a
candidate has no RTK capability; it never silently falls back to `raw`.
`provider exec go -- version` also uses this plan executor; a raw Windows Go
run returned exit code 0 in the local verification, while a forced unavailable
RTK adapter returned exit code 127 without starting a child process.

## Initial generic resolver proof

The on-demand dispatcher is now a deliberately bounded generic mechanism for
`go`, `cargo`, `node`, `npm`, `pnpm`, `python`, `python3`, `pytest`, `java`,
`gradle`, `mvn`, `dotnet`, and `git`. It uses the same discovery, verified path
mapping, `ExecutionPlan`, and output-adapter selection as Go. Other command
names retain the v0.3 static route and are never interpreted as shell syntax.

On Windows, discovery accepts only launchable executable suffixes when `where`
returns several wrappers: `.exe`, `.com`, `.cmd`, or `.bat`. This prevents an
extensionless POSIX-style npm shim or a PowerShell script from becoming a
Windows execution candidate when `npm.cmd` is also present. A live local
`doctor npm --refresh` selected `npm.cmd`, detected npm 11.16.0, and both
`provider exec npm -- --version` and normal `npm --version` returned exit 0.

Cargo is the second live proof: a process-contract fixture hides Windows Cargo,
exposes a temporary Cargo binary only in WSL through `RTK_WSL_EXTRA_PATH`,
forces the raw adapter, and verifies `rtk-wad cargo --version` reaches it. A
table-driven fixture extends that same WSL-only proof to Node/npm/pnpm,
Python/python3/pytest, Java/Gradle/Maven, .NET, and Git. Each executable is
resolved by name, launched at its discovered WSL path, and must return its own
path marker. This proves no tool-specific process launcher was added.

An unknown legacy route no longer prevents this resolver from selecting a
verified generic provider. A live Windows Node check previously inherited the
conservative WSL1 fallback; it now selects `windows-raw` through the resolver
when the Windows Node candidate and project path are verified. The raw Node
process contract also sends Ctrl+Break to a non-writing `node -e` child and
verifies prompt cancellation.

This does not claim that every runtime matrix is complete: the controlled WSL2
fixtures cover Windows-origin calls, same-distro WSL-origin calls, and one
Windows-mounted other-distro WSL route. Real installations, WSL1, native
Linux-path other-distro routing, and Windows-binary compatibility beyond the
Windows-mounted Go case remain additional matrix evidence. Their absence does
not prevent a raw-capable alpha from dispatching available providers.

If a Windows toolchain is verified but the legacy adaptive preference would
have selected native RTK, the dispatcher falls back to the verified Windows
binary with the `raw` adapter when RTK is absent. This is intentional: binary
location is decided before output adaptation, and a missing optional adapter
must not turn a runnable command into exit `127`.

### Initial dogfood observation (2026-07-26)

The local debug binary was exercised with an isolated dispatcher state
directory and no project mutation. These are single-run process observations,
not release benchmarks:

| Workspace | Command | Cold / warm | Route | Result |
| --- | --- | --- | --- | --- |
| `rtk-wad` | `cargo --version` | 2963 ms / 3523 ms | discovered Windows raw | Cargo 1.97.0, exit 0 |
| Flowpeek (`flow-explore`) | `npm --version` | 2206 ms / 1520 ms | validated Windows raw | npm 11.16.0, exit 0 |
| kas-new (`kas`) | `npm --version` | 1931 ms / 1508 ms | validated Windows raw | npm 11.16.0, exit 0 |

The Cargo observation exposed and then verified the Windows raw fallback above.
The output-adapter token delta is intentionally unreported for raw commands;
no RTK adapter ran, no fallback occurred in the final runs, and no cancellation
was requested. These startup samples predate the later per-hit version
revalidation and are therefore not current latency evidence. Full dogfood
still needs representative build/test commands, repeated samples, cache-hit
telemetry, and cancellation records.

### Second dogfood smoke cycle (2026-07-26)

An isolated state directory was used again after the cache-version and Windows
wrapper fixes. These are read-only smoke workloads, not a performance claim.
Each cold/warm pair returned identical UTF-8 output hashes and exit 0:

| Workspace | Command | Cold / warm | Output SHA-256 | Route |
| --- | --- | --- | --- | --- |
| `rtk-wad` | `cargo metadata --no-deps --format-version=1` | 2887 ms / 3537 ms | `4ca11c2a…f13ea5e6` | discovered Windows raw |
| Flowpeek (`flow-explore`) | `npm run` | 11153 ms / 8690 ms | `da242ab7…db4ef6fd` | discovered Windows raw |
| kas-new (`kas`) | `npm run` | 9456 ms / 9272 ms | `79c96e89…5a72aa1b` | discovered Windows raw |

No RTK adapter was available, so raw-versus-RTK token deltas, adapter fallback,
and RTK cancellation are explicitly not claimed. Together with the first
version smoke cycle, this is two successful non-mutating dogfood cycles; alpha
dogfooding still requires representative build/test workloads, repeatable
cache telemetry, and cancellation evidence.

### Third dogfood route-repeat (2026-07-26)

The current local release build from dirty worktree parent
`4523ec411b72df58e1db17a1538016a1472884d8` repeated an intentionally
non-mutating command in the two external corpus repositories. The dispatcher
state stayed under this repository's ignored `target/` directory. Flowpeek was
clean before and after the run; kas retained its pre-existing untracked
`.project-flow/` directory unchanged. These timings are route-repeat evidence,
not a benchmark.

| Workspace | Command | First / second | Output SHA-256 | Route and adapter |
| --- | --- | --- | --- | --- |
| Flowpeek (`flow-explore`) | `npm --version` | 1896.4 ms / 1877.0 ms | `165cd838734f79eda32ac408b099381deb3fef2148d6ef6179c954604238cfc7` | Windows raw / raw |
| kas-new (`kas`) | `npm --version` | 1967.0 ms / 2162.1 ms | `165cd838734f79eda32ac408b099381deb3fef2148d6ef6179c954604238cfc7` | Windows raw / raw |

Every invocation exited `0` and returned npm `11.16.0`. `--explain-route`
reported `command manifest selects the validated Windows raw provider`; no RTK
adapter, token calculation, fallback, installation, or RTK cancellation
occurred. Raw Node cancellation is covered separately by the process contract.
The run deliberately does not claim a cache hit because this manifest route
does not require on-demand provider discovery.

The same release build then ran Flowpeek's representative non-mutating
`npm run test:fast` workload through that raw route. It exited `0` in
75,822.8 ms at the dispatcher boundary; Flowpeek reported 195 passing tests,
zero failures/cancellations/skips, and an internal duration of 72,629.9554 ms.
The Flowpeek worktree remained clean afterwards. This supplies a functional
dogfood result, not RTK-adapter or cancellation evidence.

kas-new's `npm test` ran through the same route with exit `0` in 79,907.1 ms.
Vitest reported 83 passing files and 378 passing tests in 73.10 seconds; the
pre-existing untracked `.project-flow/` directory remained the only status
entry afterwards. The runner emitted existing React warnings about forwarded
`asChild` props and nested buttons, but no test failed, cancelled, or skipped.
Those application warnings are recorded as corpus output, not attributed to
the dispatcher.

### Adaptive fallback and repeat dogfood (2026-07-27)

The post-alpha working tree adds a regression test for the adaptive invariant:
an unavailable WSL1 Go probe is removed from candidate selection while a
verified Windows raw Go plan remains executable. A second test retains a
verified WSL2 plan as a pre-start fallback when WSL1 was selected first. The
full Windows/WSL process contract passed 28 of 28 tests, including the
dispatcher-owned version command and the existing PowerShell, CMD, Git Bash,
WSL bridge, cache, and cancellation cases.

The release build then repeated two non-mutating dispatcher workloads:
Flowpeek's `npm run test:fast` and kas-new's `npm test`. Both exited `0`
through `route=raw` with `output_adapter=raw`; kas-new reported 83 passing
test files and 378 passing tests. Flowpeek stayed clean. kas-new retained only
its pre-existing untracked `.project-flow/` directory. Existing application
warnings in kas-new were not attributed to RTK-WAD.

Set `RTK_WAD_OUTPUT_ADAPTER=raw` to explicitly disable RTK output adaptation.
The default `auto` value preserves v0.3.0 preference for an available RTK
adapter. `rtk` requires a verified RTK candidate.

Diagnostics are read-only:

```powershell
rtk-wad doctor go
rtk-wad which go
rtk-wad --explain-route go version
```

`doctor` and `which` report Windows and every eligible WSL distribution,
binary identity, version/capability probe, usable project mapping, candidate
reason, and recommendation. `--explain-route` prints the selected route and
`output_adapter`.

WSL discovery applies the configured `RTK_WSL_EXTRA_PATH` before probing both
the requested binary and RTK. This keeps discovery and the later WSL launcher
on the same executable search boundary.

Discovery cache schema v3 fingerprints `PATH`, `PATHEXT`, configured provider
inputs, WSL distribution inventory, and the Git revision of the current
project. A fingerprint change is a cache miss; `--refresh` remains available
for an explicit miss. Cached Windows and WSL binary identities are rechecked
before a hit is accepted, and the cached tool version is re-probed before a
hit is accepted (`--version`, then the conventional `version` fallback). This
keeps Node-style and Go-style tools covered. A live fixture also changes a WSL
Go version while preserving its size and mtime and verifies the next lookup is
a miss. Separate temporary fixtures prove `miss`, then `hit`, then `miss`
after a Git HEAD revision, `PATH`, or configured distro changes. Version
revalidation is deliberate correctness work; the cache still avoids repeating
full all-distro discovery. Version and capability data remain in the diagnostic
record.

## Deliberately deferred

- Applying a provisioning plan or installing Go.
- Product rename.
- RTK benchmark evidence, WSL1, native-Linux-path other-distro WSL, and
  Windows-binary compatibility coverage for every supported ecosystem.

Those are follow-up evidence or product-scope decisions; they do not change
the raw dispatcher fallback contract.

## P7 exit audit

This table is the release decision boundary for this branch. `Covered` means
there is a current automated or recorded process proof; it does not turn an
unavailable external provider into a synthetic pass.

| Original requirement | Evidence | Status |
| --- | --- | --- |
| Freeze v0.3.0 executable behavior and comparison | Immutable tag, pinned P18 blobs, unit/process regression suites | Covered locally; P20 benchmark evidence remains separately strict |
| Separate request, discovery, route, adapter, execution, provisioning contracts | `dispatcher` module and plan executor; provisioning remains uninvoked | Covered |
| `doctor`, `which`, and `--explain-route` show provider decisions | Live WSL-only Go fixture checks all three, candidate and diagnosis | Covered |
| Go from Windows shell when installed only in WSL | PowerShell, CMD, Git Bash live fixtures; literal argv, mapped CWD, exit 42 | Covered for Windows → WSL2 |
| Go Windows / WSL1 / WSL-origin / cross-distro matrix | Windows native observed; WSL-origin same-distro, one Windows-mounted `docker-desktop` → Ubuntu cross-distro route, and Ubuntu → compatible Windows Go are covered through `rtk-wad-wsl.sh`; WSL1 and native-Linux-path cross-distro candidates are unavailable and diagnosed without error | Covered for available providers |
| Process contract: structured argv, stdio, exits, cancellation, CWD | Existing raw/WSL tests plus shell matrix, stdin, WSL Ctrl+Break, raw Windows Node Ctrl+Break, and dispatcher-owned version contract | Covered for available Windows/WSL2 surface |
| Cache invalidation: PATH, binary, distro, version, Git revision | Context fingerprint plus identity and version revalidation; live same-identity version, temporary Git revision, PATH, and configured-distro fixtures | Covered for available providers |
| Resolver across Rust, Node, Python, Java, .NET, Git | Shared resolver; live WSL2 fixture for every listed executable; Windows npm wrapper proof | Covered for Windows → WSL2 |
| Dogfood on rtk-wad, Flowpeek, kas-new | Four recorded non-mutating cycles; Flowpeek and kas-new workloads pass through raw dispatch with route, exit, and corpus-status observations | RTK-adapter/cancellation evidence pending; raw pre-start fallback is unit-covered |
| v0.4.0-alpha.2 functional readiness | Four real dogfood cycles, raw fallback, 28/28 process contract, and local release build | Ready for a clean-worktree publication decision; RTK benchmark evidence is not claimed |
