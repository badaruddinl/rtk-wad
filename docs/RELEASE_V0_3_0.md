# v0.3.0 release record

`v0.3.0` makes RTK-WAD the single supported command and distribution target.
It is an intentional early-release breaking change: `rtk-wsl` and `rtk-wsl1`
are removed rather than retained as aliases.

## One command contract

- The sole executable and installer target is `rtk-wad.exe`.
- WSL1 and WSL2 remain selectable through `rtk-wad --route wsl1` and
  `rtk-wad --route wsl2`.
- Release archives contain only `rtk-wad.exe`.
- Existing `v0.2.1` assets remain immutable historical evidence and are
  superseded by this release; they are not the current installation path.

## Release conditions

The immutable tag must point at a `master` commit that passes hosted quality and
packaging gates, the self-hosted Windows/Ubuntu WSL process contract, and the
provenance workflow that publishes the archive, SHA-256 sidecar, and GitHub
attestation.
