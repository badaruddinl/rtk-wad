# WSL1 Bridge Validation Record

Date: 2026-07-24  
Branch: `feature/wsl1-native-bridge`  
Baseline: stable `v0.1.0` at `ba86a740d318a4e790414c84446165d372de749e`

## Host Capability Result

The host runs Microsoft Store WSL 2.5.9.0 on Windows 11 build 26200. Existing
Ubuntu distributions are WSL2 and were not converted, terminated, or modified.

The isolated import used Canonical's official Ubuntu 22.04 WSL image:

```text
File: ubuntu-jammy-wsl-amd64-ubuntu22.04lts.rootfs.tar.gz
Bytes: 341130963
SHA-256: 1483CC5C1DCE13064F774834CBFFDFF226559FD522A67A381A8EA77D63FB4109
```

The checksum matched Canonical's published `SHA256SUMS`.

The first import attempt targeted `E:\luthfi\wsl\Ubuntu-RTK-WSL1` and failed
before registration:

```text
Wsl/Service/RegisterDistro/0xd000000d
```

The `E:` volume is exFAT and reports `Full Repair Needed`. WSL1 expands its root
filesystem into host files and requires NTFS semantics; exFAT is not a supported
runtime location. No target distro or populated install directory remained after
the failed attempt. The corrected runtime target is
`%LOCALAPPDATA%\rtk-wsl\Ubuntu-RTK-WSL1` on the healthy NTFS `C:` volume.

The corrected NTFS import succeeded without changing Windows Features:

```text
Name: Ubuntu-RTK-WSL1
Version: 1
Default user: rtk
Home: /home/rtk
RTK: 0.43.0
```

The provisioner completed twice, proving that its verification and provisioning
path is idempotent.

## Bridge Implementation Result

The release build is installed as an independent command:

```text
Path: C:\Users\badaruddinl\.local\bin\rtk-wsl1.exe
Bytes: 284672
SHA-256: 8EF93500A2F24914EC3E3D2C78E0BBFFB560CF9EB37A7FAD8351F31AF6200FA4
```

The normal `rtk-wsl.exe` installation was not replaced.

Observed diagnostics:

```text
bridge=rtk-wsl
backend=wsl1
distro=Ubuntu-RTK-WSL1
detected_wsl_version=1
git_mode=auto
```

The diagnostic command passed against the registered isolated distro.
Explicitly overriding the alias to `RTK_WSL_BACKEND=wsl2` selected the existing
`Ubuntu` WSL2 distro and passed. Selecting the WSL1 backend with that WSL2 distro
returned a version-mismatch failure.

Native Git routing remained operational independently of the WSL backend:

```text
rtk-wsl1 git --version
git version 2.50.1.windows.1
```

## Completed Gates

| Gate | Result |
|---|---|
| Rust unit tests | 13 passed |
| Windows/WSL2 process tests | 6 passed |
| Windows/WSL1 process tests | 6 passed |
| Clippy with warnings denied | Passed |
| Release build | Passed |
| Packaging and recovery, including `rtk-wsl1.exe` | Passed |
| PowerShell syntax validation | Passed |
| Cargo package content audit: 20 expected files | Passed |
| Native Git route independent of WSL backend | Passed |
| Backend override and mismatch diagnostics | Passed |

Initial WSL1 process execution passed literal argv, Unicode, stdout/stderr, exit
codes, and stdin. The first parallel-lock run exposed a WSL1-specific platform
behavior: a second concurrent `wsl.exe --exec` session returned an internal
`EventFd` error before Linux locking could run.

The bridge now acquires a Windows named mutex before WSL1 launch. It also places
the WSL1 child in a separate Windows process group and terminates only the
dedicated distro on cancellation. The cancellation deadline, queued-command
continuation, and lock release regression all passed without leaving a child
process behind.

The WSL1-specific integration suite is opt-in through
`RTK_WSL1_TEST_DISTRO=Ubuntu-RTK-WSL1` and was executed against the registered
WSL1 distro.

## Benchmark Result

The measured corpus was Flowpeek at commit `d31c959`. Each path received one
warm-up run followed by five measured runs. Values below are medians in
milliseconds.

| Workload | Raw Windows | Windows RTK | `rtk-wsl` WSL2 | `rtk-wsl1` WSL1 |
|---|---:|---:|---:|---:|
| `git status --short` | 122.228 | 273.592 | 138.221 | 144.617 |
| `git log -100 --oneline` | 119.693 | 181.718 | 133.294 | 148.827 |
| Large `rg` query | 112.778 | 190.722 | 1053.935 | 412.716 |

Both bridge aliases routed the Git workloads to native `git.exe`, so their small
differences are wrapper variance rather than Linux backend performance. For the
Linux RTK `rg` workload, WSL1 was 60.8% faster than WSL2 but remained 116.4%
slower than native Windows RTK and 266.0% slower than raw ripgrep.

Raw ripgrep produced 67,668 bytes across 456 lines. WSL1 RTK produced 20,230
bytes across 205 lines, a 70.1% byte reduction and approximately 11,859 fewer
estimated tokens. All paths returned exit code zero.

After terminating only the dedicated WSL1 distro, the first WSL1 query completed
in 714.932 ms and the immediately repeated query completed in 332.841 ms. The
existing WSL2 `vmmemWSL` working set did not increase during either query; WSL1
does not allocate a separate utility VM.

## Promotion Decision

The WSL1 profile is runtime-correct and all implementation gates are green. It is
ready for opt-in dogfooding as a supported experimental profile. It does not
replace the default WSL2/native-Git route because native Windows RTK is still
faster for the measured noisy-search workload and WSL1 cancellation deliberately
terminates the dedicated distro.
