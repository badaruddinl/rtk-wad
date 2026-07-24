# Filesystem and local-state matrix (2026-07-24)

## Environments

| Worktree | Volume | Filesystem | Purpose |
| --- | --- | --- | --- |
| `E:\luthfi\project\rtk-wsl` | `E:` | exFAT | Real source worktree, including an uncommitted test-file change. |
| Temporary Git repository under the local Codex work directory | `C:` | NTFS | Fresh Windows-native comparison worktree. |

The WAD executable used a different local application-data directory on `C:`
for each case. Its stock native RTK path was explicit, so the result does not
depend on ambient `PATH` discovery.

## Results

| Check | exFAT source | NTFS temporary worktree |
| --- | --- | --- |
| `rtk-wad --explain-route git status` | `native-rtk`, exit 0 | `native-rtk`, exit 0 |
| `rtk-wad git status --short --branch` | exit 0 | exit 0 |
| Ledger at `%LOCALAPPDATA%\rtk-wad\metrics-v1.sqlite` on `C:` | Present | Present |
| Remaining tracker scratch entries | 0 | 0 |
| `.rtk-wad` created in source/worktree | No | No |

The Windows process-contract suite also starts the WSL bridge from a temporary
Windows worktree and verifies that `/bin/pwd` receives the matching
`/mnt/<drive>/...` current directory. The existing cancellation/lock test runs
from the source worktree and verifies Ctrl+Break release before its five-second
deadline.

## Conclusion

WAD does not require an NTFS source worktree. Native route selection and local
metrics work consistently on the tested exFAT and NTFS locations, while SQLite
state remains on the local Windows volume. The temporary benchmark worktrees
are intentionally outside the repository and are not release artifacts.
