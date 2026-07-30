# Deterministic Windows/WSL CI

GitHub-hosted Windows runners do not provide a controlled Ubuntu WSL
distribution. The hosted `windows-ci.yml` therefore proves Rust, package, and
installer contracts but must not claim that it proved the WSL process boundary.

`windows-wsl-self-hosted.yml` contains separate required deterministic WSL1 and
WSL2 jobs. Register a
Windows self-hosted GitHub runner with the labels `self-hosted`, `Windows`, and
`WSL`, install `Ubuntu` as WSL2, and provision `Ubuntu-RTK-WSL1` through
`scripts/provision-wsl1.ps1`. Each job parses `wsl.exe --list --verbose` and
fails unless its distro has the requested version. The WSL1 job additionally
requires the root-owned dedicated-runtime marker before it runs the complete
`process_contract` integration suite through the actual WSL1 route.

The workflow is manual/callable rather than attached to every push so a missing
private runner cannot leave public pull requests queued forever. A stable
release requires both recorded successful runs in addition to hosted CI.
