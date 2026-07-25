# Deterministic Windows/WSL CI

GitHub-hosted Windows runners do not provide a controlled Ubuntu WSL
distribution. The hosted `windows-ci.yml` therefore proves Rust, package, and
installer contracts but must not claim that it proved the WSL process boundary.

`windows-wsl-self-hosted.yml` is the required deterministic job. Register a
Windows self-hosted GitHub runner with the labels `self-hosted`, `Windows`, and
`WSL`, install a current Ubuntu WSL distro, and ensure its RTK provider is
configured before dispatching the workflow. The job fails if `Ubuntu` is absent
and then runs the complete `process_contract` integration test suite.

The workflow is manual/callable rather than attached to every push so a missing
private runner cannot leave public pull requests queued forever. A stable
release requires its recorded successful run in addition to hosted CI.
