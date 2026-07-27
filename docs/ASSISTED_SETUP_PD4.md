# Assisted Go setup planning: PD4

PD4 adds an explicit, local-first setup planner:

```powershell
xuva setup go
xuva setup go --json
xuva setup go --refresh
```

The command never installs, downloads, elevates, changes `PATH`, or starts a
package manager. It is deliberately a planning boundary, not a bootstrap
shortcut.

## Plan states

`setup go` produces one of three states.

| State | Meaning | Proposed action |
| --- | --- | --- |
| `ready` | A complete existing provider can run Go. | None. |
| `planned` | A Windows project has native RTK, no Windows Go, and `winget` is available. | Show the exact GoLang.Go `winget` command for review. |
| `blocked` | The required provider contract is incomplete or the target is unsafe. | No installer is selected. |

A planned command is evidence for a later explicit apply milestone; it is not
an instruction that WAD executes in PD4. The plan always includes the required
post-change check: `xuva doctor go --refresh`.

## Safety contract

The planner does not use a Windows installer from a WSL project, does not pick
an alternate package manager when `winget` is absent, and does not propose Go
when native RTK is missing. It also leaves WSL toolchain installation out of
scope: selecting a distro or package manager without an explicit user choice
would create ambiguous state.

The next milestone may add an apply transaction only after its confirmation,
installer-result, re-discovery, rollback, and cancellation contracts are
specified and tested. PD4 therefore reports `apply=unavailable_in_pd4` for a
reviewable plan instead of making an implicit change. An attempted `--apply`
request is rejected with that same boundary; it cannot fall through to an
installer.

## Validation

Deterministic tests cover a Windows `winget` proposal, an already-ready
provider, and blocked states. Runtime validation uses the existing local Go
provider and checks that planning returns `ready` without launching an
installer. A missing-provider scenario may be simulated by withholding the Go
search path; its result is a plan or a block only, never an installation.
