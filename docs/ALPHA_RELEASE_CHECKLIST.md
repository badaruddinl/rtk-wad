# 0.1.0-alpha.1 freeze checklist

Use this checklist before declaring the local alpha a stable baseline or creating
an upstream contribution. Check an item only with current evidence.

## Technical gate

- [x] Structured argv reaches WSL without rebuilding a user shell command.
- [x] Windows drive CWD maps to the matching `/mnt/<drive>/...` directory.
- [x] Explicit custom WSL CWD is supported through `RTK_WSL_CWD`.
- [x] Spaces, quotes, ampersands, semicolons, dollar signs, backslashes, and
  Unicode are covered by the process contract.
- [x] Stdout, stderr, interactive stdin, and exit codes 0/1/42/127 are covered.
- [x] Ctrl+Break forwards SIGINT to the launched Linux process group and releases
  the shared lock without terminating the distro.
- [x] Parallel-lock regression is covered by the process contract.
- [x] Configuration rejects empty or relative Linux-path values and invalid lock
  wait values.

## Packaging and recovery gate

- [x] Fresh install, refusal without `-Force`, upgrade backup, rollback, uninstall,
  `.cmd` fallback, and failed-install preservation pass in a temporary destination.
- [x] Installer stages the new executable before moving the active executable.
- [x] Active workstation launcher has been upgraded and prior binaries are retained
  as named backups.
- [x] `cargo package` archive audit excludes `target/`, workstation configuration,
  and raw logs.

## Quality gate

- [x] `cargo fmt --all --check`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test -- --test-threads=1` (10 tests)
- [x] `cargo build --release`
- [x] `cargo package --allow-dirty`

## Dogfooding gate

- [x] Two concise release-binary cycles completed on `rtk-wsl` and Flowpeek.
- [x] Failure, fallback, and latency-outlier evidence is recorded without raw logs.
- [x] Complete two normal development work cycles with the active installed binary
  and review the record for recurring fallback or lock contention (post-freeze
  monitoring; it does not block this technical baseline).
- [ ] Run the Windows CI workflow once and review its WSL runner result. The first
  run exposed `WSL_E_DISTRO_NOT_FOUND` on the hosted runner; the workflow now
  reports this capability gap without hiding it as a source failure.

## Publication decision gate

- [x] Apache-2.0 is the selected license for this standalone companion repository.
- [x] `badaruddinl/rtk-wsl` is a standalone companion project, not an official
  upstream RTK artifact.
- [x] Tag `v0.1.0-alpha.1` exists; a GitHub Release with binary assets remains
  intentionally deferred until post-freeze dogfooding and CI review.
- [ ] Keep the upstream issue/PR closed until the above decisions are explicit.

## Freeze decision

Frozen as the technical `0.1.0-alpha.1` baseline on 2026-07-24 after the owner
accepted the remaining dogfooding work as post-freeze monitoring. The publication
decisions above remain explicit follow-ups; this freeze does not authorize an
upstream RTK contribution.
