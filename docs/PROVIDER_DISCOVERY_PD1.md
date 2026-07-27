# On-demand provider discovery: PD1

PD1 adds read-only discovery for the exact Go tool name. It does not change the
normal XUVA dispatcher, invoke a package manager, download anything, request
elevation, or offer an installation prompt. Existing commands therefore retain
their prior routing contract.

## Commands

```powershell
xuva resolve go
xuva resolve go --json
xuva resolve go --refresh
xuva doctor go
```

`resolve` exits successfully even when Go is absent, because it is a diagnostic
query. `doctor` exits unsuccessfully when no safe provider is available. Both
commands only accept the exact tool name `go` in PD1. `--refresh` bypasses the
local cache for that one query.

## Discovery contract

For a Go query, WAD checks the Windows executable through `where.exe` and
checks each eligible installed WSL distribution with a fixed `command -v` probe
for `go` and `rtk`. The probe does not interpolate user input into a shell
script. `docker-desktop` and `docker-desktop-data` are intentionally excluded:
they are system-managed WSL distributions, not general development providers.

The result records the current project locality, each inspected WSL provider,
Windows RTK availability, candidate reasons, and a recommendation index. It is
an availability diagnosis only. PD1 never changes dispatch based on that index.

## Local cache

Discovery writes only `%LOCALAPPDATA%\xuva\provider-cache-v1.json`, or the
equivalent root selected by `XUVA_STATE_DIR`. Entries are scoped to the
requested tool and expire after five minutes. A cache hit avoids every Windows
and WSL executable probe. A missing provider is also cached, so an unavailable
Go binary does not repeatedly start WSL discovery.

The cache contains executable paths, WSL distribution names and versions, and
timestamps. It never contains command arguments, project content, credentials,
or command output.

## Deliberate PD1 limits

Windows projects with a Go binary only in WSL are shown as discovered but not
usable yet; PD2 will validate the actual Windows-to-WSL path mapping before a
route can use that provider. A WSL project may use only a Go provider in the
same WSL distribution during PD1. Windows execution from a WSL project is also
reported but deliberately not selected.

Assisted installation belongs to a later milestone. When no provider exists,
PD1 reports `install=disabled_in_pd1`; it does not attempt a fallback install.
