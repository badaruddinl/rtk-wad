# Deterministic Windows/WSL CI

GitHub-hosted Windows runners do not provide a controlled Ubuntu WSL
distribution. The hosted `windows-ci.yml` therefore proves Rust, package, and
installer contracts but must not claim that it proved the WSL process boundary.

`windows-wsl-self-hosted.yml` contains separate required deterministic WSL1 and
WSL2 jobs. It runs on every same-repository pull request and protected-branch
push without path filtering, so documentation-only changes cannot bypass a
required check; it also runs nightly and remains manually/callably available.
Register a Windows self-hosted GitHub runner with the labels `self-hosted`,
`Windows`, and `WSL`, install `Ubuntu` as WSL2, and provision
`Ubuntu-RTK-WSL1` through `scripts/provision-wsl1.ps1`. Each job parses
`wsl.exe --list --verbose` and
fails unless its distro has the requested version. The WSL1 job additionally
requires the root-owned dedicated-runtime marker before it runs the complete
`process_contract` integration suite through the actual WSL1 route.

For security, code from an external fork is not executed automatically on the
private self-hosted machine. A maintainer must first review it and reproduce the
commit on a trusted same-repository branch before the required WSL evidence can
exist. Stable branch protection must require both named jobs; hosted CI's
opportunistic WSL probe is not a substitute for either result.
