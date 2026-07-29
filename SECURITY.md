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
