# Security policy

## Supported versions

XUVA is currently a public beta. Security fixes are applied to the latest
published release line and the current `master` branch.

## Reporting a vulnerability

Do not open a public issue for credential exposure, command execution,
cross-host environment leakage, release provenance, or path-routing defects.
Use GitHub's **Report a vulnerability** private security-advisory flow for
`badaruddinl/xuva`.

Include the affected version, operating system and WSL version, exact command
shape with secrets removed, expected boundary, observed behavior, and a minimal
reproduction when possible. The maintainer will acknowledge a complete report
within seven days and coordinate remediation and disclosure through the private
advisory.

## Security boundaries

XUVA preserves structured argv and never intentionally reconstructs a shell
pipeline. Cross-host children receive an isolated environment with a documented
allowlist. Unknown executables are identity-discovered but are never executed
for diagnostic version probing. Releases are accepted only from immutable tags
that pass the controlled Windows/WSL gate on the same source SHA.

WSL launches are cancellation-gated: the Linux launcher first establishes and
attests its cancellation boundary, and only then can the Windows parent publish
a matching permit. WSL2 process-group tokens are stored below a private
per-user runtime directory rather than directly in shared `/tmp`. Active tokens
are never deleted by age. The parent retains cancellation state after a Windows
proxy exit and accepts Linux cleanup only after a verified dead process group
or an identity-matched completion attestation. A WSL1 child validates and
attests the root-owned, read-only dedicated-runtime marker in the same session
before the parent can permit execution. Cancellation accepts that exact
installation ID and requires it to match the marker again immediately before
`wsl.exe --terminate`. After authorization, the WSL1 launcher remains as a
process-group supervisor. It publishes completion only after same-group
descendants are gone, and the Windows parent requires the attested status to
match the proxy status. Missing, malformed, or contradictory completion is a
fail-closed lifecycle error that revalidates and resets only the dedicated
WSL1 distro before releasing the global mutex.

XUVA supervises foreground process groups; it is not a daemon service manager.
A child that intentionally creates a new Linux session is outside this
boundary and must have a separately managed lifecycle.

Provider discovery records path, size, and modification time. The current
public-beta threat model trusts the selected user's local toolchain and writable
`PATH` directories after discovery; XUVA does not yet hash or code-sign every
provider again immediately before spawn. Treat untrusted shared tool
directories as outside the supported security boundary.
