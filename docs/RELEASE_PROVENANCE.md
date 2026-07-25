# Release provenance and Windows signing readiness

`release-provenance.yml` builds an existing immutable tag on GitHub-hosted
Windows, produces the two Windows compatibility executables in a ZIP, writes a
SHA-256 sidecar, publishes the assets, and creates a GitHub build provenance
attestation for the archive. It is manual so an ordinary branch build can never
publish an artifact.

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
