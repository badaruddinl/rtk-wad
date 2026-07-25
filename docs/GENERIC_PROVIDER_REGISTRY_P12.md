# P12 generic provider registry

## Scope

P12 replaces the Go-only discovery boundary with a generic, local-first
provider registry. `rtk-wad resolve <tool>` and `rtk-wad doctor <tool>` now
accept a validated executable name and inspect existing Windows and eligible
WSL providers without installing, starting a shell command, or changing route
selection.

P12 is intentionally discovery-only. Automatic cross-host execution remains
limited to the already verified Go contract until P13 proves bidirectional
project mapping for generic providers.

## Cache schema and identity

The provider cache uses schema version 2 and is stored as
`provider-cache-v2.json`. Every discovered provider records a
binary identity when it can be read:

```text
path + size_bytes + modified_unix_seconds
```

For Windows this comes from file metadata. For WSL it comes from `stat` in the
target distribution, with the executable path passed as a structured argument.
RTK binaries receive the same treatment. The cache has a bounded TTL and is
replaced atomically. A deliberate refresh rebuilds all identities; therefore a
tool or RTK replacement cannot silently reuse a previous identity.

The registry does not run `tool --version` during normal discovery. P11 found
Windows shims visible through WSL that either fail under WSL1 or have CRLF
interpreter failures. Calling arbitrary version commands would make discovery
slow and could create a new hang surface. P18 benchmark evidence may attach
semantic version data under a separately bounded execution contract.

## Safe tool names

The generic interface accepts only 1–128 character ASCII names containing
letters, digits, `.`, `_`, or `-`. Paths, whitespace, shell metacharacters,
and non-ASCII names are rejected before Windows or WSL discovery begins.

```powershell
rtk-wad resolve python --json
rtk-wad doctor cargo --refresh
rtk-wad resolve "tool;not-run"
```

The first two commands inspect providers. The third command fails validation;
it is never passed to `where.exe`, WSL, or a shell.

## Compatibility

Existing Go resolution and the explicit `setup go` transaction keep their
contracts. Schema v1 cache data is treated as stale and rebuilt rather than
partially migrated. This is safe because the cache is disposable discovery
metadata and contains no installation state.

## P12 acceptance gates

- Generic tool-name validation is covered by unit tests.
- WSL binary identity parsing is covered without retaining command output.
- Existing Go discovery, execution, setup, and process contracts still pass.
- `resolve` and `doctor` have no installer path for any generic tool.
- Flowpeek is refreshed after source edits; static limitations remain explicit.

P13 will add verified Windows-to-WSL and WSL-to-Windows project mappings before
generic provider candidates can affect automatic execution.
