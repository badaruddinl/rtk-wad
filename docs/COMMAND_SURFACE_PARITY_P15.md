# Native RTK command-surface parity: P15

P15 makes the complete upstream RTK command inventory a runtime contract for
XUVA. The embedded source of truth is
`benchmarks/command-manifest.json`, whose reviewed compatibility allowlist
currently contains only RTK `0.43.0`.

```powershell
xuva surface
xuva surface --json
```

The JSON report lists every upstream top-level command, its classification, and
its default auto-route. It is diagnostic only; it does not run a tool, discover
a provider, or modify local state.

## Classification policy

| Classification | Default XUVA behavior |
| --- | --- |
| `native_structured` | Stock Windows RTK with structured argv. `git` remains subcommand-aware: read-only forms may use RTK; mutations use raw Git once. |
| `raw_native` | Existing Windows toolchain directly, once. |
| `wsl1_conservative` | Isolated WSL1 RTK until a command-specific Windows contract exists. |
| `core_internal` | Handled by XUVA's own diagnostic and ledger interface. |

The same manifest owns the disjoint Git read-only and mutation subcommand sets;
runtime classification consumes those sets directly. Unknown subcommands fail
closed as unknown and use the mutation-safe raw boundary. The manifest parser
rejects unknown fields, duplicates, overlap, malformed names, or a compatibility
set that omits the primary adapter version.

Compatibility is an explicit reviewed allowlist bound to the embedded protocol
and capability contract. XUVA does not assume that an arbitrary newer patch or
minor version is compatible merely because its semver is numerically close.

XUVA-owned `dart` and `flutter` shims remain explicit Windows raw extensions;
they are not represented as upstream RTK commands. Unknown commands retain the
conservative WSL1 route and are visibly classified as unknown rather than
silently receiving native RTK treatment.

## Drift and parity evidence

The process contract starts the actual Ubuntu RTK `0.43.0` binary, parses its
`--help` command list, and requires exact set equality with `xuva surface
--json`: 69 command families, no duplicate names, and no unknown
classification. The existing PowerShell manifest verifier remains available
when a stock Windows `rtk.exe` is installed:

```powershell
.\benchmarks\verify-command-manifest.ps1 -NativeRtk C:\tools\rtk.exe
```

This is inventory parity, not a claim that every external command has a live
benchmark row. P18 supplies the reproducible raw/native/XUVA benchmark evidence
required before a performance or token-saving claim is published.

## Relationship to provider execution

P14 proves that a verified Windows or WSL provider can execute safely through
an explicit boundary. P15 deliberately does not promote arbitrary unknown
commands to that boundary automatically. The manifest makes every upstream
family explicit first; P16 can then combine these safety classes with measured
provider and route evidence without changing the command vocabulary silently.
