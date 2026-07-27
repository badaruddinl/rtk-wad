# Setup operational freeze: PD6

PD6 freezes the Go-provider and opt-in setup contract for local alpha use. It
does not publish, tag, push, install Go, or change a user's global PATH.

## Repeatable readiness gate

`tests/setup-readiness-contract.ps1` runs a release binary under the temporary
name `xuva.exe` and a private `XUVA_STATE_DIR`. It verifies:

1. `setup go --status` is read-only.
2. A ready provider makes `--apply --confirm` a no-op rather than an install.
3. An unconfirmed apply exits `2` and creates no transaction journal.
4. Recovery with no journal is read-only.

The script restores its environment and removes only its own GUID-named
temporary directory. It never invokes `winget`. A host without an already-ready
Go provider skips the ready-provider no-op assertion instead of attempting a
setup.

Run the gate after a release build:

```powershell
.\tests\setup-readiness-contract.ps1 -Source .\target\release\xuva.exe
```

## Frozen operator contract

| Operation | Side effect |
| --- | --- |
| `resolve go`, `doctor go`, `setup go` | Discovery/cache only. |
| `setup go --apply` | Fresh plan, exit `2`, no installer. |
| `setup go --apply --confirm` | May run only the documented Windows `winget` command when the fresh plan is safe. |
| `setup go --status` | Read-only journal view. |
| `setup go --recover` | Fresh discovery and journal update only; no replay. |

The first public alpha should be released only after a maintainer runs this
gate, the Rust quality gates, the package contract, and the command-manifest
check from a clean working tree. The release decision, remote push, and tag are
deliberately separate from this local freeze.
