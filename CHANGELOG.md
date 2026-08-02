# Changelog

All notable changes to XUVA are documented here. The project follows semantic
versioning, and release artifacts are built from immutable version tags by the
GitHub release-provenance workflow.

## [Unreleased]

### Changed

- Split CLI parsing, policy resolution, execution planning, and process running
  into typed, independently testable stages while keeping the raw front door
  allocation-free.
- Made local metrics explicitly opt-in, bounded, purgeable, ACL-protected, and
  free of raw command and error text.
- Moved isolated benchmark state to an ACL-protected LocalAppData location and
  added a CI contract for current XUVA environment names.
- Replaced the README's cross-run optimization comparison with a fresh 160-run
  public-corpus sample and explicit claim boundaries.

### Security and reliability

- Enforced library tests in CI and release gates; made unknown WSL versions
  fail closed and revalidated provider identity immediately before launch.
- Added bounded policy validation, transactional cross-process state updates,
  complete fallback discovery after unusable partial WSL inventory, and
  regression coverage for each boundary.

## [0.4.4] - 2026-08-01

### Added

- Added a security-first product and engineering guideline covering trust
  boundaries, UX expectations, modularity, testing, and release discipline.
- Added clearer CLI help, policy inspection, calibration inspection, routing
  explanations, diagnostics, setup, self-update, and WSL lifecycle surfaces.
- Added explicit build identity and provenance output through
  `xuva --version --verbose`.

### Changed

- Reduced `src/main.rs` to a minimal runner and split the former monolithic
  application into bounded CLI, execution, provider, routing, state, setup,
  diagnostics, self-update, and WSL modules.
- Separated provider discovery, verification, mapping, resolution, dispatch,
  and cache policy so changes remain locally testable and lower-risk.
- Isolated adaptive routing policy, calibration, and decision ownership with
  fail-closed behavior for incompatible or invalid evidence.
- Updated package metadata to the canonical `badsleepyday/xuva` repository.

### Performance

- Removed stable raw-route policy and calibration reads from eligible command
  fast paths.
- A historical host run reported lower auto-routing medians for representative
  `git` and `rg` workloads than an earlier run. That cross-run comparison is
  retained as historical evidence, not causal proof or a universal guarantee.

### Security and reliability

- Hardened atomic state writes and secret environment filtering.
- Bound RTK provider selection to verified manifest identity.
- Restored fail-closed calibration behavior and expanded process, routing,
  provider, cache, CLI, and integration contracts.
- Kept official release publication behind locked dependencies, MSRV checks,
  Rust quality gates, packaging recovery tests, SHA-256 sidecars, SBOM output,
  and GitHub build-provenance attestations.

[0.4.4]: https://github.com/badsleepyday/xuva/compare/v0.4.3...v0.4.4
