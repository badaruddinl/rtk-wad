# Draft upstream proposal: Windows-to-WSL bridge

Status: local draft only. Do not open an issue or pull request until the upstream maintainers confirm that this belongs in RTK rather than in user documentation.

## Licensing and contribution status

The proof of concept is licensed under Apache-2.0 to match upstream RTK and is marked `publish = false`; it does not claim the upstream repository URL as its own.

Upstream's current contribution guide requires focused changes, tests, documentation, a PR targeting `develop`, and CLA completion. Its referenced `CLA.md` was not present at the documented path during the 2026-07-24 audit, so the actual CLA Assistant text must be reviewed when a PR is opened. If an employer may own the work, the contributor must obtain the permission required by the upstream contribution terms before submission.

## Problem

RTK's Windows documentation correctly recommends WSL for full hook support, while native Windows is intentionally limited. The documented WSL workaround uses a shell command and `env -i`. A Windows launcher that constructs a shell string can lose argument boundaries before RTK receives them, especially for spaces, `;`, `&`, `$`, and backslashes. It can also start in the WSL home directory instead of the caller's Windows worktree.

## Proposed direction

Consider an optional Windows bridge command or documented companion launcher that:

1. invokes `wsl.exe -d <distro> --cd <mapped-cwd> --exec ...`;
2. passes the RTK command as process argv, never as a reconstructed shell string;
3. starts RTK with an explicit clean Linux environment when that remains necessary for hooks;
4. preserves child exit status; and
5. keeps existing WSL-native and native-Windows modes unchanged.

The local proof of concept is intentionally separate from RTK. It uses `flock` because the existing local analytics database is shared, but upstream should decide whether locking belongs in the product, a helper, or documentation.

## Evidence from the local alpha

- A structured launcher preserved literal `semi;and&dollar$HOME` and `C:\Program Files\Example` through WSL argv.
- `wsl.exe --exec` alone started relative commands in the Linux home directory; adding `--cd /mnt/<drive>/...` restored relative `read`, Git, and search behavior from Windows worktrees.
- Linux exit status 42 was observed as Windows exit status 42.
- A deterministic lock probe confirmed that a second launcher waits after the first acquires the shared lock.

These are workstation observations, not cross-machine performance claims. Startup timing varied significantly, so no latency promise should be inferred.

## Acceptance criteria for an upstream implementation

- Tests cover spaces, ampersands, semicolons, dollar signs, and Windows backslashes as individual argv elements.
- A command launched from a Windows-drive working directory observes the matching `/mnt/<drive>/...` CWD.
- Nonzero exit status is preserved.
- No shell interpolation occurs in the bridge.
- Native Windows behavior and the documented WSL-native workflow remain unchanged.

## Non-goals

- Replacing the Unix-shell hook model.
- Claiming native Windows hook parity.
- Translating arbitrary UNC paths, custom WSL mount layouts, or Windows tool invocations in the first iteration.
- Automatically modifying users' agent hook configuration.

## Relevant upstream context

- [RTK Windows documentation](https://github.com/rtk-ai/rtk#windows) recommends WSL for full support.
- [Discussion #1212](https://github.com/rtk-ai/rtk/discussions/1212) documents a WSL hook workaround and the need for `env -i`.
- [Discussion #671](https://github.com/rtk-ai/rtk/discussions/671) records native-Windows hook friction.
