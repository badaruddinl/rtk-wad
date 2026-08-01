# XUVA product and engineering guideline

This document is the canonical product, architecture, security, command-line
experience, delivery, and performance guideline for XUVA. It consolidates the
product formulation, technical audit, cross-platform vision, roadmap, and the
maintainer decisions that govern current development.

When an implementation choice conflicts with this guideline, resolve the
conflict in this order:

1. security and process-contract integrity;
2. correctness and fail-closed behavior;
3. a clear, predictable command-line experience;
4. measured performance;
5. implementation convenience.

Performance work must never weaken a security boundary, hide uncertainty, or
skip evidence required to prove process completion.

## Product definition

XUVA is a unified command execution layer that routes commands to verified
environments while preserving their process contract.

The primary interaction remains:

```text
xuva <command> <arguments>
```

XUVA answers seven questions:

1. Where is the requested command available?
2. Which provider is compatible, trusted, and reachable from this project?
3. Should execution be native, cross-host, or adapter-backed?
4. Which arguments are paths, and how may those paths be mapped safely?
5. Which environment values may cross the execution boundary?
6. How are stdin, stdout, stderr, cancellation, descendants, and exit status
   preserved?
7. Why was the route selected, and what can the user do when it is unavailable?

XUVA unifies execution environments. It does not attempt to unify shell
languages.

### Name and positioning

XUVA means **eXecution. Unified. Verified. Adaptive.**

The primary tagline is:

> Unified execution. Verified routes.

For Windows and WSL:

> Run Windows and WSL tools through one safe command boundary.

For the longer-term product:

> Run commands in the right local, isolated, or remote environment without
> changing your terminal.

## Product boundaries

XUVA owns:

- command and provider discovery;
- route selection and explanation;
- structured argv forwarding;
- conservative path mapping;
- environment isolation;
- execution planning and supervision;
- cancellation and descendant cleanup;
- stdout, stderr, stdin, and exit-status forwarding;
- adapter compatibility;
- local calibration and evidence-backed optimization;
- provider diagnosis and trust reporting.

XUVA is not:

- a terminal emulator or a new shell;
- a universal PowerShell/Bash parser;
- a package manager, container orchestrator, or service manager;
- a replacement for Docker, Podman, mise, direnv, Nix, or Distrobox;
- permission to execute a command twice for calibration or fallback.

Shell operators remain owned by the invoking shell. XUVA accepts an executable
and structured arguments; it must not rebuild an ordinary command as a shell
string.

## Security and engineering attitude

Security is a design input, not a final review step. Engineering decisions must
be conservative, evidence-backed, reviewable, and explicit about limitations.

Required attitudes:

- prefer a small, provable contract over broad implicit behavior;
- fail closed when lifecycle, identity, mapping, or completion evidence is
  missing or contradictory;
- never hide an unfavorable benchmark or an unsupported environment;
- distinguish static evidence, runtime evidence, and human decisions;
- avoid cleverness on a security boundary;
- keep changes narrow so reviewers can reason about one contract at a time;
- do not mix behavior changes with mechanical code movement;
- preserve user changes and never silently broaden authority;
- use explicit diagnostics instead of surprising fallback;
- treat credentials, release identity, and provider identity as high-risk data.

### Structured process contract

The core invariant is:

```text
command = executable + argv + cwd + environment + process policy
command != shell string
```

Arguments remain structured and must not be re-quoted into a general shell
command. A cross-host bridge may encode the structured fields, but its supported
encoding boundary must be documented precisely. The current Windows/WSL bridge
guarantee is limited to supported UTF-8 arguments; it must not be described as
arbitrary byte-perfect Unix argv.

### Environment boundary

Cross-host execution starts from an isolated environment. Only documented safe
baseline variables and explicitly allowed non-credential variables may cross.

Credential-like names are rejected even when present in a general allowlist.
Credential forwarding, if ever added, requires a separate explicit mechanism,
clear user confirmation, redacted diagnostics, and dedicated threat modeling.

No diagnostic output, trace, cache, or test fixture may persist secret values.

### Provider identity and trust

Discovery proves availability at an observation point; it does not permanently
authorize later execution. Provider identity must be revalidated immediately
before spawn. A mismatch invalidates the cached candidate and stops execution.

Managed providers require content-digest verification. Shared writable
locations must be reported as a weaker trust boundary. Signature verification
may strengthen the contract but never replaces path, architecture, capability,
and project-mapping validation.

### Path mapping

Do not translate every string that resembles a path. Translate only arguments
whose command contract identifies them as paths and whose destination mapping
can be verified. Revision names, patterns, URLs, opaque data, and shell syntax
must remain untouched.

### Process lifecycle

Cross-host execution is fail closed:

- a child cannot start before its cancellation boundary is ready;
- Ctrl+C is forwarded according to the documented escalation contract;
- process groups and descendants are supervised;
- proxy exit alone is not proof that the target process completed;
- completion status must be attributable to the authorized invocation;
- a command is never replayed after process start;
- WSL1 recovery may affect only a revalidated dedicated runtime.

## Architecture

`src/main.rs` is a binary shim, not an application module. Its only
responsibility is to call the library application runner and terminate with the
returned exit status.

Target shape:

```text
src/main.rs
    -> xuva::app::run_from_env().terminate()

src/
|-- app.rs                 top-level orchestration
|-- cli/                   parsing, commands, help, diagnostics, output
|-- routing/               classification, policy, calibration
|-- providers/             model, discovery, cache, trust, validation
|-- execution/             plans and platform adapters
|-- lifecycle/             supervision and cancellation
|-- contracts/             argv, environment, paths, process invariants
|-- state/                 metrics, calibration, provider/setup state
`-- self_update.rs         update lookup and release identity
```

Architecture rules:

- the binary declares no duplicate modules and owns no business logic;
- each contract has one implementation and one owning module;
- module APIs expose the smallest useful surface;
- platform-neutral policy does not depend on WSL implementation details;
- filesystem, clock, environment, and process-spawn dependencies have testable
  seams where they affect policy;
- unit tests live near their owning contract;
- integration tests verify the complete binary and process boundaries;
- source movement is separated from behavior changes;
- every extraction removes the old copy in the same change.

## Command-line user experience

The default UX must be understandable without reading source or documentation.
The common case remains short:

```text
xuva git status
xuva pytest -q
xuva cargo test
```

### Output principles

- Lead with the outcome, then show supporting evidence.
- Use plain language before internal terminology.
- Keep stable labels and ordering.
- Use two spaces for each indentation level; never mix tabs and spaces in
  human-readable output.
- Put command output on stdout and XUVA diagnostics on stderr.
- Do not add decorative noise to successful ordinary execution.
- Color may reinforce meaning only when stderr is an interactive terminal;
  text and exit status must carry the complete meaning without color.
- Paths, provider IDs, commands, and user-controlled values must be rendered
  unambiguously and safely.
- Never print secrets or full sensitive environment values.

An explained successful route should follow this shape:

```text
Resolved command
  Command: pytest -q
  Route: wsl2
  Provider: Ubuntu
  Adapter: raw
  Working directory:
    Windows: C:\work\project
    Provider: /mnt/c/work/project
  Environment: isolated
  Reason:
    pytest is unavailable on Windows and verified in Ubuntu WSL2.
```

An error should state the failure, evidence, and next action:

```text
error: no verified provider can run `pytest`
  Checked:
    Windows: command not found
    Ubuntu: project path is not reachable
  Next:
    Run `xuva doctor pytest` for provider diagnostics.
```

### Command UX contract

- `--help` is task-oriented, concise, and includes examples.
- `--explain-route` explains the selected route without obscuring the command
  result.
- `which`, `resolve`, and `doctor` answer progressively deeper questions and
  use the same terminology.
- explicit user route/provider choices override adaptive policy or fail with a
  precise explanation; they do not silently degrade.
- machine-readable output uses an explicit versioned JSON schema and emits no
  unrelated prose on stdout.
- invalid syntax identifies the exact problem and prints the nearest valid
  usage.
- exit status remains the command's status after execution; pre-start XUVA
  failures use documented XUVA statuses.

## RTK adapter contract

RTK is optional. Raw execution remains the safe default when adapter value or
compatibility is not verified.

The versioned manifest is the single source of truth for:

- adapter version and protocol;
- command classification;
- raw native commands;
- conservative cross-host commands;
- internal commands;
- mutation subcommands.

Unknown manifest fields are rejected. Categories cannot overlap. Adapter
availability requires a compatible version, protocol, and capability set, not
merely an executable with the expected name.

Git mutations in a Windows worktree remain pinned to native Windows Git unless
a future contract explicitly proves an equally safe alternative. Adaptive
policy cannot override this invariant.

## Performance

Correctness, security, modularity, and observability are completed before the
final adaptive optimization phase. Performance work must be measurement-driven
and must not become an early delivery blocker.

The raw hot path performs no unnecessary WSL discovery, version probe, adapter
startup, metrics database access, policy write, or cache refresh. Metrics-off
means zero SQLite access.

Final adaptive tuning uses interleaved direct-command/XUVA benchmarks with cold
and warm scenarios reported separately. Target budgets are:

```text
pure adaptive decision p50 < 50 us
pure adaptive decision p99 < 200 us
warm provider resolution p50 < 0.25 ms
warm provider resolution p95 < 1 ms
hot raw incremental overhead p50 <= 5 ms
hot raw incremental overhead p95 <= 10 ms
```

Absolute zero overhead is not a truthful promise for an additional process.
The practical goal is an incremental cost inside benchmark noise or the budgets
above, with no safety regression. Unfavorable measurements remain published.

## Delivery sequence

Work proceeds in this order:

1. establish this guideline and current repository identity;
2. restore a clean, observable verification baseline;
3. create one library application runner and reduce `main.rs` to a shim;
4. extract provider, routing, state, CLI, execution, and lifecycle ownership in
   small behavior-preserving changes;
5. harden manifest and provider identity contracts in separate reviews;
6. finish the zero-discovery raw fast path;
7. pass formatting, lint, unit, integration, Windows/WSL process, packaging,
   provenance, and release gates;
8. only then profile and tune adaptive routing to the measured noise floor.

No new platform provider is added until the Windows/WSL product, architecture,
security boundaries, and hot path are stable.

## Verification and completion

A source tranche is complete only when:

- formatting and warnings are clean for its scope;
- relevant unit and integration tests pass;
- CLI text and exit-status compatibility are intentionally verified;
- security-sensitive negative tests remain present;
- Flowpeek is refreshed and changed contexts/impact are reviewed;
- unsupported runtime or parser areas are stated explicitly.

The overall goal is complete only when the minimal runner, modular ownership,
security hardening, CLI UX contract, full verification matrix, and final
adaptive performance gates are all satisfied.
