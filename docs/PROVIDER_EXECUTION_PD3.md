# Provider-aware Go execution: PD3

PD3 connects the existing Go provider discovery to normal `rtk-wad go ...`
execution. It remains installation-free: no package manager, download,
elevation, prompt, or dependency action is available in this milestone.

## Conservative selection

For automatic routing only, WAD keeps the established Windows route whenever a
usable Windows Go binary exists. If Windows Go is unavailable, it may select a
WSL provider only when all of the following are true:

1. Go is present in that distribution.
2. RTK is present in that same distribution.
3. The distribution reports WSL 1 or WSL 2.
4. The project is local to that distribution, or a Windows project has a
   successful structured `wslpath -a` mapping there.

The selected distribution, backend, RTK path, and Linux current directory are
carried into the existing WSL execution contract. No command is replayed after
a child starts.

An explicit `--route` remains authoritative. A command containing an explicit
Linux path remains on its existing WSL route rather than being reclassified.

## Missing provider behavior

When no safe provider satisfies the contract, automatic `rtk-wad go ...`
returns exit code `127` before starting a child process. Its diagnostic directs
the user to `rtk-wad doctor go` and explicitly states that installation is
disabled in PD3. This preserves the normal distinction between discovering a
missing binary and executing a command that failed.

## Validation

The PD3 runtime smoke test confirmed that an available Windows Go installation
continues through the raw Windows route and reports its native version. With
both the Windows Go search path and native RTK override withheld, the same
automatic request returned `127` with the missing-provider diagnostic before a
child process started. The current machine has no complete WSL Go+RTK provider,
so WSL provider selection is covered by a deterministic unit fixture together
with the PD2 cross-host path-mapping contract; PD3 does not install Go merely
to create an end-to-end fixture.

## Current limits

PD3 does not yet run a WSL Go binary without RTK, select a Windows Go binary
from a WSL project, or install any component. Those cases remain diagnostic
results, not implicit fallback paths. Token and latency policy continue to
govern native Windows RTK only when the required Windows toolchain exists.
