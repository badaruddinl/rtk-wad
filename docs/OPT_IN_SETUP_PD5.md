# Opt-in Go setup transaction: PD5

PD5 turns the PD4 plan into a deliberately narrow Windows apply transaction.
It still does not install anything during discovery, routing, `doctor`, or a
plain `setup go` request.

## Commands

```powershell
xuva setup go
xuva setup go --apply
xuva setup go --apply --confirm
xuva setup go --status
xuva setup go --recover
```

`--apply` refreshes discovery and renders the current plan again, then exits
with code `2`. Only `--apply --confirm` may start an installer. It does so only
when the fresh plan is `planned`: the project is on Windows, native RTK is
already verified, Go is absent, and `winget` is available.

The sole installer command is structured rather than shell-composed:

```text
winget install --id GoLang.Go --exact --source winget --accept-package-agreements --accept-source-agreements
```

No WSL package manager or alternate Windows installer is guessed. A ready
provider performs no action; a blocked plan cannot be applied.

## Transaction and recovery

Before the installer starts, WAD atomically writes a local transaction journal
at `%LOCALAPPDATA%\xuva\setup-transaction-v1.json` (or the test/developer
override `XUVA_STATE_DIR`). The journal records only the command, status,
timestamp, and concise result. It contains no source code or project data and
is not intended for Git.

After a successful installer exit, WAD performs fresh provider discovery. It
marks the transaction `verified` only when a complete provider is visible. If
the installer fails or a cancellation leaves the process interrupted, the
journal remains evidence of the outcome. `setup go --recover` then performs
fresh discovery and updates the journal to `recovered_verified` or
`recovery_required`; it never replays an installer.

There is intentionally no automatic uninstall or rollback. Package-manager
uninstall could remove a user-managed Go installation or lose version-specific
state, so recovery is detection-first and non-destructive. Manual removal, if
needed, remains an explicit user action through the package manager.

## Validation boundary

The repository tests the decision and recovery contracts without invoking
`winget`. Runtime smoke tests cover `ready`, a simulated `planned` state, and
the non-confirming `--apply` path; they never pass `--confirm`. This preserves
the local workstation while proving that no installer can be reached without
the final explicit command.
