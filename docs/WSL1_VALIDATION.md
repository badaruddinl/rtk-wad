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
Bytes: 287744
SHA-256: 347C06F9B7834ED8E4C4362E836E9A4986CA41EBC61781051F34660A63519C08
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
| Rust unit tests | 14 passed |
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

## Native Windows RTK and Optimized WSL1 Result

RTK 0.43.0 source and a controlled ten-case argument probe identified two
distinct contracts:

- structured `rtk proxy` and specialized commands preserve argument boundaries;
- `rtk run` reconstructs a command string and invokes `cmd /C` on Windows;
- single-string `rtk proxy` reparses its value with a POSIX-style splitter.

Structured proxy preserved 10 of 10 cases. Single-string proxy preserved 5 of
10, while both `run` forms preserved 3 of 10. Failures included spaces, empty
arguments, Windows paths, environment expansion, and command metacharacters.

Layer profiling showed that WSL1 already had a Windows named mutex and
dedicated-distro cancellation, making Linux `setsid` and `flock` redundant. The
optimized WSL1 path removes only those redundant layers.

On the same Flowpeek query, one warm-up and five measured runs produced:

| Path | Median |
|---|---:|
| Raw Windows ripgrep | 100.307 ms |
| Native Windows RTK | 181.467 ms |
| Direct WSL1 RTK | 291.241 ms |
| Previous WSL1 bridge | 371.894 ms |
| Optimized WSL1 bridge | 306.070 ms |
| WSL2 bridge | 918.083 ms |

The optimized bridge was 17.7% faster than the previous WSL1 bridge in the
layer-isolation run and remained within 14.829 ms of direct WSL1 RTK.

The final four-workload benchmark used `o200k_base` rather than the prior
four-bytes-per-token estimate. The optimized WSL1 path reduced the
`graphVersion` search from 16,466 to 5,086 tokens, saving 11,380 tokens (69.1%),
with median latency improving from 416.933 to 315.482 ms. The broad source query
fell from 248,209 to 5,689 tokens, saving 242,520 tokens (97.7%), with median
latency improving from 478.106 to 401.736 ms.

## Promotion Decision

The WSL1 profile is runtime-correct and all implementation gates are green. It is
ready for opt-in dogfooding as a supported experimental profile. It does not
replace the default WSL2/native-Git route because native Windows RTK is still
faster for the measured noisy-search workload and WSL1 cancellation deliberately
terminates the dedicated distro.
