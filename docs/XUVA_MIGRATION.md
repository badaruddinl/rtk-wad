# XUVA hard cutover

Starting with `v0.4.1`, XUVA is the only product identity and launcher:

- Repository: `github.com/badsleepyday/xuva`
- Command and Windows binary: `xuva` / `xuva.exe`
- WSL origin shim: `scripts/xuva-wsl.sh`
- Configuration: `XUVA_*` and `XUVA_WSL_*`
- Local state: `%LOCALAPPDATA%\xuva`

`rtk-wad` is not distributed, installed, or documented as a supported command
in this release line. GitHub redirects the former repository URL so historic
tags, releases, and evidence remain reachable. The short-lived `v0.4.0`
release was transitional; use `v0.4.1` or later for the complete XUVA identity.
