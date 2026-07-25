# Go, Dart, and Flutter adapter benchmark (2026-07-24)

## Scope and method

This record uses three real workloads across two local Windows worktrees. `go test ./...` ran in
`E:\luthfi\project\go-practice`; Dart format checking and Flutter analysis ran
in `E:\luthfi\project\Flutter\foodp`. Every measured variant completed five
rotated warm samples. Latency is end-to-end wall-clock time and token counts use
`o200k_base` over combined stdout and stderr.

The Go comparison contains raw Windows Go, stock Windows RTK 0.43.0, and WAD in
auto mode. Dart and Flutter are WAD-owned Windows shims rather than upstream
RTK 0.43.0 top-level commands, so their valid comparison is raw Windows versus
WAD; treating stock RTK's unsupported-command error as performance data would
be misleading. The Go policy run adds an explicit WAD-native candidate to
measure the actual selected route. The runner retained raw output hashes and normalized only each
tool's documented elapsed-time field before semantic comparison.

WAD state was isolated with `RTK_WAD_STATE_DIR`. This leaves the child tool's
normal Windows `LOCALAPPDATA` and caches untouched. Each process had a
60-second deadline; no measured process timed out.

## Results

| Workload | Raw Windows median | Stock RTK median | WAD auto median | WAD native candidate median | Raw / WAD-auto tokens | Native / WAD-native tokens | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `go test ./...` | 991.334 ms | 845.351 ms | 981.023 ms | 1,107.436 ms | 24 | 7 | Native output saved 17 tokens (70.8%); WAD auto remained byte-identical to raw. |
| `dart format --set-exit-if-changed lib test` | 3,699.567 ms | N/A | 3,778.909 ms | N/A | 15 | N/A | Raw and WAD semantic output matched; WAD overhead was 79.342 ms (2.1%). |
| `flutter analyze` | 8,443.255 ms | N/A | 8,533.201 ms | N/A | 20 | N/A | Raw and WAD semantic output matched; WAD overhead was 89.946 ms (1.1%). |

All measured commands exited `0`. Flutter's first independent cold analysis
took 70.2 seconds before the normal cache was ready; it is reported as a cold
start observation, not included in the warm median. The controlled runner
warm-up then completed successfully before the five recorded samples.

## Adaptive decision

The generated five-sample policy is limited to the exact `go test ./...` argv
form. It includes a fourth, explicit WAD-native candidate so its 1,107.436 ms
latency includes the dispatcher and local-accounting cost. Its 70.8% token
reduction exceeds the documented 25% threshold, so WAD selects native RTK for
that form despite the local 116.102 ms median latency cost. `go test` without
`./...`, and every other Go command, remain on
the validated raw Windows route. The policy was imported into an isolated WAD
state and dogfooded successfully; `--explain-route` reported `native-rtk` for
the exact form and `raw` for `go test`.

Dart and Flutter remain raw Windows shims. They preserve the caller's native
toolchain and produce no token reduction for these successful compact outputs;
there is no upstream RTK command to promote. The measured overhead is small but
not a speed win, so the result is compatibility evidence rather than a claim
that WAD accelerates the SDKs.

No local .NET project (`.sln` or `.csproj`) was available during the corpus
inventory. `dotnet` therefore remains a tested compatibility fallback only; it
has no published latency or token-saving claim until a pinned real .NET corpus
is available.

The machine-readable outputs and the generated policy remain local and ignored
by Git because they contain workstation paths, timings, and local state.
