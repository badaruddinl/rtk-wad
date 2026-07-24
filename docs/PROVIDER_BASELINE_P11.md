# P11 provider and command-surface baseline

## Purpose

P11 begins the universal RTK-WAD journey with repeatable evidence rather than a
single Go-specific assumption. The provider audit identifies Windows tools,
WSL tool providers, every discovered RTK binary, RTK command surfaces, and
currently running WAD/WSL processes without installing, changing, or executing
the candidate toolchains.

Run the audit locally:

```powershell
.\scripts\audit-provider-baseline.ps1 `
  -OutputPath .\.flowpeek\cache\p11-provider-audit.json
```

Use `-ProbeToolMetadata` only when a full version/hash pass is needed. The
default records every tool path and fully probes only RTK providers. Metadata
probing has a three-second budget per distro by default; raise
`-MetadataBudgetSeconds` deliberately for an offline audit. An unhealthy
Windows shim visible from WSL therefore cannot make normal discovery slow.

`.flowpeek` is intentionally local-only and ignored by Git. The report records
the project-independent machine evidence needed by later milestones; it is not
a source of runtime correctness or a reason to select a route by itself.

## Current workstation observation

The initial P11 inspection on 2026-07-25 found these RTK providers:

| Host | Provider | RTK status |
| --- | --- | --- |
| Windows | PATH/configured executable search | No stock `rtk.exe` was available. `rtk-wsl.exe` was present as the compatibility launcher, not misclassified as an RTK provider. |
| Ubuntu (WSL2) | `/usr/local/bin/rtk` | `rtk 0.43.0` |
| Ubuntu-22.04 (WSL2) | `/usr/local/bin/rtk` | `rtk 0.43.0` |
| Ubuntu-RTK-WSL1 | PATH | No RTK provider discovered. |

Windows had existing Go, Node, Python, .NET, Dart/Flutter, Git, and ripgrep
providers. The two WSL2 distributions exposed native Git, ripgrep, and Python,
while several Windows shims observed through WSL were not automatically usable.
In particular, the Flutter shell shims exposed CRLF interpreter failures and
the Windows NPM shim reported WSL1 as unsupported. These are capability facts,
not fallback signals.

The audit also found unrelated long-running WSL processes for another project.
They were deliberately not terminated: WAD may only terminate child processes
it created or processes explicitly placed in scope by the user.

## Command-surface evidence

The committed command manifest declares RTK `0.43.0` and provides the current
classification baseline. It must be compared against `rtk --help` from every
discovered RTK provider. A mismatch is a P11 finding, not a reason to silently
reuse a stale manifest.

P11 does not yet claim that every command is executable on every host. It
proves inventory and classification only. P15 will turn each command family
into a capability and process-contract matrix, with fixtures for missing local
toolchains.

## P11 exit criteria

- Provider audit is repeatable and local-first.
- Every discovered RTK binary includes host, distro, version, hash, and parsed
  command surface.
- Windows discovery limitation is explicit; a whole-disk scan needs explicit
  roots and `-DeepSearch`.
- Existing command manifest differences are machine-readable.
- Process audit reports only; it never kills unscoped work.
- Flowpeek graph is refreshed before and after P11 source edits, and its static
  limitations remain documented.

## Queue after P11

P12 consumes the audit output to replace the Go-only cache with a generic,
version-aware provider registry. P13 then proves path mappings in both
directions. No automatic cross-host execution is enabled before P13 has a
verified mapping contract.
