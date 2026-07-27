# Bidirectional provider mapping: P13

P13 turns provider locality into a verified diagnostic contract. It still does
not enable generic cross-host execution; P14 is the separate execution gate.
No installer, package manager, shell command string, or route selection is
introduced by this milestone.

## Required proof

Every candidate has a project path in the candidate host's syntax only after
both conditions succeed:

1. `wslpath` is started through `wsl.exe -d <distro> --exec` with every path as
   one argument.
2. The target host confirms that the resulting path is an existing directory.

When `XUVA_WSL_USER` selects a WSL user, both probes include `-u <user>`. The
mapping is therefore evaluated under the same WSL identity that a later
provider execution would use.

For a Windows project and WSL provider, WAD asks the provider distribution:

```text
wsl.exe -d <distro> --exec wslpath -a <Windows path>
wsl.exe -d <distro> --exec test -d <mapped Linux path>
```

For a WSL project and Windows provider, WAD asks the source distribution:

```text
wsl.exe -d <distro> --exec wslpath -w -a <Linux path>
```

It then verifies the returned Windows drive path or the matching
`\\wsl.localhost\<distro>\...` UNC path with the Windows filesystem. A UNC
mapping that names another distribution, an unreadable result, an unknown
project location, or a cross-distro Linux project is rejected.

This covers ordinary drive mounts, custom WSL mounts reported by `wslpath`, and
native WSL filesystems when Windows exposes the matching UNC share. It does not
assume `/mnt/<drive>` and does not treat a successful text conversion as a
working directory proof.

## Scope boundary

`resolve` and `doctor` may now report a verified Windows candidate for a WSL
project. That information is diagnostic only. Existing Go routing keeps its
more conservative P3 rule, and generic tools are not dispatched cross-host
until P14 proves execution, cancellation, exit-code, and argument contracts.

Mappings are intentionally rechecked during `resolve` and `doctor` rather than
stored as durable routing state. Mounts and UNC availability can change without
the tool binary changing; P16 may introduce a bounded cache only with explicit
freshness and invalidation evidence.

## Acceptance gates

- Unit tests cover both structured `wslpath` directions, spaces, Unicode, and
  literal shell characters.
- Unit tests reject wrong-distro UNC output and unreadable destination paths.
- Runtime process contracts prove a WSL project maps to a Windows provider for
  Windows-mounted and native paths on WSL2, plus WSL1 when its dedicated test
  distribution is provisioned.
- Existing provider discovery and Go routing contracts continue to pass.
- Flowpeek is refreshed after source edits. Its static graph is not used as
  runtime evidence.
