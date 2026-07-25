# Benchmark matrix preflight: P18

P18 begins with a provider and command-surface preflight. This prevents a
benchmark report from comparing a raw Windows tool to an unavailable, outdated,
or differently shaped RTK provider and then calling the result a WAD win.

Run the local-only audit before a benchmark session:

```powershell
.\scripts\audit-provider-baseline.ps1 `
  -OutputPath .\.flowpeek\cache\p18-benchmark-preflight.json
.\tests\benchmark-preflight-contract.ps1
```

An isolated provider is intentionally not added to PATH. Supply it explicitly
when auditing that provider:

```powershell
.\scripts\audit-provider-baseline.ps1 `
  -SearchRoots "$env:LOCALAPPDATA\rtk-wad\benchmark-providers\v0.43.0\windows" `
  -WslRtkOverride "Ubuntu-RTK-WSL1=/home/rtk/.rtk-wad-benchmark/v0.43.0/rtk" `
  -OutputPath .\.flowpeek\cache\p18-benchmark-preflight.json
```

The override format is `Distro=/absolute/linux/path`. It is validated against
the registered WSL distributions and never changes PATH, a distro default, or
the normal WAD configuration.

The ignored JSON report records the complete 69-command manifest, each
discoverable Windows RTK candidate, every WSL RTK candidate, their version and
help exit codes, command-set equality, and readiness for native Windows, WSL1,
and WSL2 evidence.

## Release-evidence rule

A three-way claim for a command family requires all of the following on the
same machine and pinned corpus:

1. Raw Windows execution with exit code, output hash, latency, and
   `o200k_base` token count.
2. A verified stock Windows RTK whose `--help` command set exactly matches the
   embedded manifest.
3. WAD execution using that exact native RTK path, with the same measurements.
4. A separate WSL1 or WSL2 row only when the selected WSL provider also matches
   the manifest and the project/provider path contract is verified.

The preflight is intentionally not an installer. `false` readiness means
benchmark evidence is blocked, not that WAD may download, select, or substitute
another provider. A WSL RTK binary is not a stand-in for the required native
Windows RTK row.

## Coverage discipline

The P15 manifest remains the source of truth for all command families. P18
does not label a family as performance-covered merely because it appears in
`rtk-wad surface`, has a fixture, or has a process-contract test. Real corpora,
deterministic external fixtures, toolchain corpora, and side-effect-sensitive
internal commands retain separate evidence tiers in
[`benchmarks/README.md`](../benchmarks/README.md). Missing prerequisites remain
visible in the generated report and are release blockers for the affected
backend claim.

Each machine-readable benchmark artifact records the exact pinned `tiktoken`
package version alongside `o200k_base`. The WAD installer owns that private
tokenizer dependency from P19 onward; benchmark scripts must not silently use a
different tokenizer or a bytes-per-token approximation.
