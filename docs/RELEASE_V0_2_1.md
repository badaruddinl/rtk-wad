# v0.2.1 release record

`v0.2.1` is the stable patch release that packages the current Windows-first
dispatcher and its deterministic Windows/WSL validation.

## Release conditions

The tag must point at the merged `master` commit after all of these conditions
are satisfied:

1. Hosted `rust-quality` and `packaging-recovery` checks are successful.
2. The self-hosted Windows runner completes the full `process_contract` suite
   against that same commit and its configured Ubuntu WSL provider.
3. The manual provenance workflow builds the immutable tag, uploads
   `rtk-wad-v0.2.1-windows-x86_64.zip` and its SHA-256 sidecar, and records a
   GitHub attestation.

## Distribution boundary

The ZIP contains `rtk-wad.exe` and the legacy-compatible `rtk-wsl.exe`. The
archive is integrity-verifiable through its SHA-256 sidecar and GitHub
attestation. It is intentionally not described as Authenticode-signed because
no code-signing certificate is configured.

## Included stabilization work

- Windows PowerShell self-hosted runner support without reliance on the
  machine's default WSL distribution.
- UTF-16-safe Ubuntu WSL discovery in Windows PowerShell.
- Portable CWD provider-contract tests, including the GitHub Actions checkout
  path rather than a developer workstation path.
