# Release provenance and Windows signing status

`release-provenance.yml` publishes only an existing immutable `v<version>` tag.
The workflow first resolves `refs/tags/<tag>^{commit}` and checks that the tag,
`Cargo.toml`, and the requested version agree. It also proves that the tagged
commit is an ancestor of protected `master` before any self-hosted runner can
execute repository code. User-controlled workflow inputs
are passed to PowerShell through step environment variables; they are never
embedded in a `run:` program.

## Build once, gate once, publish the same bytes

The hosted Windows build job checks out the resolved commit, builds one release
binary, packages it once, and uploads one immutable artifact set:

- `xuva-v<version>-windows-x86_64.zip`;
- its SHA-256 sidecar;
- a CycloneDX SBOM;
- exact toolchain provenance.

Two controlled Windows/WSL jobs download those exact bytes, verify their digest
and embedded source identity, expand the archive, and run the WSL2 and WSL1
process contracts against the packaged `xuva.exe` itself. The WSL1 gate proves
the actual distro version and dedicated marker. Both jobs are protected by the
reviewer-controlled `release-gate` environment and return the gated artifact
digest. The protected publish job downloads the same artifact;
it does not compile or package again. It rechecks the tag-to-commit mapping and
digest, creates GitHub build-provenance attestations, and publishes those exact
files. Prerelease tags are explicitly published as prereleases and never become
Latest. Existing releases/assets are rejected, and release files are never
overwritten. Candidate artifacts are retained for 14 days so a delayed approval
does not silently invalidate the gate.

Repository settings must protect both `release-gate` and `release`
environments with required reviewers. The self-hosted runner should be
ephemeral or cleaned after every job and must not retain repository credentials.

The release compiler is Rust `1.97.1`. `cargo-audit` is pinned to `0.22.2` and
`cargo-cyclonedx` to `0.5.9`. GitHub Actions are pinned by full commit SHA. The
crate's separately tested minimum supported Rust version is `1.95.0`.

## Package integrity

The ZIP contains an exact allowlisted file set, including the launcher,
installer lifecycle library, installer, uninstaller, WSL shim, optional
tokenizer installer and pin,
`RELEASE-METADATA.json`, and `SHA256SUMS`. `verify-package.ps1` checks:

- no missing or unexpected package files;
- exact checksum coverage and every payload digest;
- package version, source commit, target, profile, and provenance;
- equality between metadata and `xuva --version --verbose`.

The installer runs that verifier before activating an official package. Each
generation is a complete dedicated bundle with a validated ownership marker.
Updates rotate the bundle atomically and retain one complete previous bundle;
rollback never swaps only `xuva.exe` while leaving newer companion scripts. A
transaction journal makes interrupted rotation recoverable, and no lifecycle
operation owns or recursively removes a shared executable directory.

## Authenticode status

XUVA public-beta archives are not Authenticode-signed. Windows SmartScreen may
therefore show an unrecognized-publisher warning. Do not bypass that warning for
an archive from an unknown source: download only from the repository release,
verify the SHA-256 sidecar and GitHub attestation, then inspect the package
metadata before installation.

The project will claim Authenticode signing only after a maintainer supplies a
protected code-signing identity, documented timestamp service, verification
procedure, and reviewer-controlled release environment. Until then, the
published checksum, attestation, immutable source tag, and exact package
verification are the explicit trust boundary.
