# P20 local release gate

P20 turns the alpha's separate quality checks into one deliberate local release
gate. It verifies the current commit only; it does not choose a version, create
a tag, push a branch, publish a GitHub Release, install a provider, or change a
global toolchain.

## Required inputs

The operator supplies the exact stock providers used for benchmark claims. They
are intentionally explicit rather than discovered from `PATH`:

- one native Windows RTK executable;
- one WSL1 distro and absolute RTK path; and
- one WSL2 distro and absolute RTK path.

This prevents a stale or unrelated RTK install from becoming release evidence.
The gate rejects a dirty worktree before it builds anything.

## Run

```powershell
.\scripts\verify-release.ps1 `
  -NativeRtk "$env:LOCALAPPDATA\rtk-wad\benchmark-providers\v0.43.0\windows\rtk.exe" `
  -Wsl1Distro Ubuntu-RTK-WSL1 `
  -Wsl1Rtk /home/rtk/.rtk-wad-benchmark/v0.43.0/rtk `
  -Wsl2Distro Ubuntu `
  -Wsl2Rtk /home/badaruddinl/.local/bin/rtk
```

The paths above are an example from the P18 evidence host, not defaults in the
script. Another machine must provide its own verified provider paths.

## What it proves

The gate runs formatting, Clippy, unit tests, a release build, the full Windows
and WSL process contract, tokenizer bootstrap and installation contracts,
installer/recovery coverage, setup readiness, a strict P18 provider preflight,
the native 69-command manifest check, `cargo package`, and archive hygiene.

P18 readiness is strict here: Windows RTK, WSL1 RTK, and WSL2 RTK must each
match the complete manifest. A false readiness value is a failed release gate,
not permission to download or substitute a provider.

## Publication boundary

A passing result makes the checked commit eligible for a human release decision.
It is not itself a public release. Version selection, branch integration, tag,
binary signing, GitHub Release creation, and upstream contribution remain
separate, auditable actions.
