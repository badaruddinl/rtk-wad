# Generic setup diagnosis: P17

P17 makes setup guidance available for every safe provider name without turning
WAD into an all-in-one installer. It preserves the local-first, on-demand
contract: ordinary routing, provider discovery, and setup diagnosis never
install a tool or a dependency.

## Commands

```powershell
xuva doctor <tool> [--json] [--refresh]
xuva setup <tool> [--json] [--refresh]
```

`doctor` reports the discovered Windows and WSL candidates, their verified
project-path mapping, a recommended candidate when one exists, and a concise
diagnosis. A missing provider makes `doctor` return a non-zero exit code after
printing the diagnosis.

`setup <tool>` is diagnostic-only for every generic tool. It reports either:

| Status | Meaning | Apply field |
| --- | --- | --- |
| `ready` | An existing verified provider can be used. | `not_needed` |
| `blocked` | No verified provider is available. | `unavailable_for_generic_tool` |

Both results have no proposed installer command. WAD does not infer a package
manager, package identifier, privilege requirement, runtime version, or
dependency chain from a generic executable name.

## Explicit installation boundary

The pre-existing `setup go --apply --confirm` transaction remains the only
installer boundary. It is intentionally Go-specific, records its own local
journal, and requires both flags. Generic setup rejects `--apply`, `--confirm`,
`--status`, and `--recover`; it does not create or modify the Go transaction
journal.

This boundary keeps a fresh machine diagnosable without turning the first WAD
invocation into a bulk installation. A user can install a missing tool through
their chosen system workflow, then run `xuva doctor <tool> --refresh` to
prove the resulting provider and project mapping.

## Verification

The Windows process contract covers a ready generic provider, a missing generic
provider, doctor output for the missing case, forced apply rejection, and the
absence of an installation journal. The test never invokes `winget` or any
installer.
