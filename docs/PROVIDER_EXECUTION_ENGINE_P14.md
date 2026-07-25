# Generic provider execution engine: P14

P14 adds one explicit, end-to-end execution boundary for the provider registry:

```powershell
rtk-wad provider exec <tool> [--candidate <index>] -- <args...>
```

The separator is required. Everything after it is passed as structured process
arguments; WAD does not construct a shell command string.

Without `--candidate`, execution uses the verified recommendation from
`rtk-wad resolve <tool>`. An explicit index is useful for diagnosis and for
comparing an already verified provider. The index must still be usable; this
command cannot force an unverified mapping into execution.

`provider exec` deliberately refreshes provider discovery before selection. It
is an explicit execution operation, so avoiding a stale tool or RTK identity is
more important than preserving the lightweight diagnostic-cache latency.

## Execution contract

| Candidate kind | Child process | Working directory |
| --- | --- | --- |
| `windows_raw` | discovered Windows executable | verified Windows project path |
| `windows_rtk` | discovered stock Windows RTK, with `<tool>` as its first argument | verified Windows project path |
| `wsl_raw` | discovered Linux executable through the existing WSL process-group bridge | verified Linux project path |
| `wsl_rtk` | discovered WSL RTK, with `<tool>` as its first argument | verified Linux project path |

WSL candidates preserve the selected distro, configured WSL user, clean
environment, cancellation contract, and WSL1 global mutex. Windows candidates
receive the verified Windows or UNC path as their actual process current
directory.

The WAD metrics ledger records provider executions. Raw providers remain
unmeasured; RTK providers retain aggregate RTK metrics when their RTK binary
reports them.

## Safety boundary

- No installer, package manager, or elevation path exists here.
- A selected child process is never retried on another provider, including a
  non-zero exit code. Its exit code, stdout, and stderr are preserved.
- WAD refuses a Windows absolute path argument for a WSL provider and a Linux
  absolute path argument for a Windows provider. P14 does not silently rewrite
  path-bearing arguments; use the verified project directory with relative
  paths.
- The candidate working directory is always revalidated by P13 before this
  command selects it.

P14 intentionally exposes this capability explicitly. P15 will map the full
native RTK command surface onto this engine after every command family has a
safe automatic-routing classification.

## Verification

The Windows process contract creates isolated fake Windows raw and RTK
providers, then verifies literal arguments, mapped CWD, exact exit codes, and
the absence of replay. It also executes the real Ubuntu WSL2 RTK Git provider
and the dedicated WSL1 raw Git provider from a Windows project, and verifies
foreign absolute-path rejection before a child starts.
