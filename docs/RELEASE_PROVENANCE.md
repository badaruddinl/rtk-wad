# Release provenance and Windows signing readiness

`release-provenance.yml` accepts an existing immutable tag only. The tag and
`Cargo.toml` version must match, the controlled Windows/WSL runner must pass the
full release gate on that exact SHA, and the hosted Windows builder then creates
the ZIP, SHA-256 sidecar, CycloneDX SBOM, and GitHub build-provenance
attestation. It is manual so an ordinary branch build can never publish an
artifact.

The current public archive is
`xuva-v0.4.1-windows-x86_64.zip`. New archives contain `xuva.exe`, the
installer, uninstaller, WSL shim, license, readme, and checksum record. The
release page, SHA-256 sidecar, SBOM, and attestation are the authoritative
distribution record. A source branch or an untagged workflow artifact is not a
stable binary release.

The project does not claim Authenticode signing until a maintainer supplies a
trusted code-signing certificate and an appropriate protected GitHub
environment. Before enabling signing, require all of the following:

1. A certificate owned by the release publisher, stored only as a protected
   GitHub secret or hardware-backed signing identity.
2. A documented timestamp service and a verification command for the published
   EXE and ZIP.
3. A protected release environment with reviewer approval and immutable tags.
4. A successful self-hosted Windows/WSL process-contract run for the same tag.

Until then, users should verify the published SHA-256 sidecar and GitHub
attestation. The absence of an Authenticode signature is explicit release
metadata, not an implied guarantee.
