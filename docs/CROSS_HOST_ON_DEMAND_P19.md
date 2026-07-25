# Cross-host resolution and on-demand dependencies (P19)

P19 closes the operational boundary between verified cross-host execution and
on-demand installation. It does not make WAD an all-in-one installer.

## Verified execution directions

Provider discovery verifies a candidate's executable identity and its project
working directory on the candidate host before `provider exec` can select it.
The following local runtime checks were performed with structured arguments and
exit code `0`:

| Project location | Selected provider | Verified working directory | Command |
| --- | --- | --- | --- |
| Windows worktree | Ubuntu-22.04 WSL RTK Git | `/mnt/e/luthfi/project/rtk-wsl` | `git --version` |
| Native Ubuntu WSL worktree | Windows Git | `\\wsl.localhost\Ubuntu\tmp\rtk-wad-p19-exec-*` | `git --version` |

The first direction used an explicit verified WSL RTK candidate. The second
used an explicit verified Windows candidate after WAD converted the native WSL
path to a Windows UNC path and confirmed that directory from Windows. Neither
run installed a tool, reconstructed a shell command, or retried another
provider after execution began.

`resolve` may list multiple usable candidates, but its recommendation remains
diagnostic. `provider exec <tool> --candidate <index> -- <args...>` is the
explicit cross-host execution boundary. Normal WAD routing retains its existing
safe command classifications and does not promote a generic tool merely because
it was discovered on the other host.

## Cache and path safety

Discovery uses bounded local cache data only when its provider identity and
project mapping remain valid. `--refresh` forces new discovery; candidate
execution also refreshes before selection. Cross-host mappings are not accepted
from text conversion alone: WAD verifies the converted directory on the target
host, rejects a wrong-distro UNC path, and never rewrites path-bearing command
arguments across hosts.

## On-demand dependencies

Generic `setup <tool>` and `doctor <tool>` remain diagnostic-only. They can
report an existing provider or a missing provider but do not infer a package
manager, package identifier, dependency chain, or privilege escalation.

WAD has no owned runtime dependency. The pinned private tokenizer
`tiktoken==0.12.0` is optional and used only by reproducible benchmarks. On a
machine without Python, the explicit `-InstallTokenizer` path exposes the
single exact `winget` plan for `Python.Python.3.12` and runs it only with both
`-InstallPython` and `-ConfirmPythonInstall`. It never installs an unrelated
tool or modifies the global Python environment.

The legacy explicit Go transaction remains separately guarded by
`setup go --apply --confirm`; it is not generalized to other tools.

## Evidence and limits

Runtime process contracts cover literal arguments, Unicode, output streams,
exit propagation, cancellation, mapping in both directions, and no replay.
Tokenizer bootstrap and packaging contracts cover the private dependency and
failure-before-launcher invariant. These results prove the selected directions
and explicit boundaries above; they do not claim that every language tool can
be installed or safely auto-routed across hosts.
