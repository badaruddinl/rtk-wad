# Cargo check three-way benchmark (2026-07-24)

The RTK-WAD source worktree was kept on `E:` while `CARGO_TARGET_DIR` was on
the Windows local NTFS volume. Each variant ran five warmed `cargo check`
samples against the same worktree and cache.

| Variant | Median ms | `o200k_base` output tokens | Exit |
| --- | ---: | ---: | ---: |
| Raw Windows Cargo | 228.480 | 24 | 0 |
| Stock Windows RTK | 318.154 | 30 | 0 |
| RTK-WAD before policy | 441.590 | 30 | 0 |

The generated evidence compares raw execution with the end-to-end WAD route and
therefore selects raw execution for `cargo:check`: it is faster and emits fewer
tokens for this warmed corpus. This is a local policy
decision, not a universal claim about Cargo diagnostics. A different project or
error-heavy build may produce different evidence and can replace the local
policy after its own benchmark run.
