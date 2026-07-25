# Cargo check four-way benchmark (P18, 2026-07-25)

The RTK-WAD source worktree was kept on `E:` while `CARGO_TARGET_DIR` was on
the Windows local NTFS volume. Each variant ran five warmed `cargo check`
samples against the same worktree and cache.

| Variant | Median ms | `o200k_base` output tokens | Exit |
| --- | ---: | ---: | ---: |
| Raw Windows Cargo | 1,678.300 | 24 | 0 |
| Stock Windows RTK | 1,777.745 | 30 | 0 |
| RTK-WAD native candidate | 1,892.819 | 30 | 0 |
| RTK-WAD auto after policy | 1,691.908 | 24 | 0 |

The P18 preflight verified the exact stock Windows RTK path and the benchmark
used `tiktoken==0.12.0`. The candidate produced 25% more output tokens and was
slower, so the context-bound v2 policy correctly chose raw execution for
`cargo:check`. Raw and final WAD output were semantically equivalent after
removing only Cargo's volatile elapsed-time field; unmodified hashes remain in
the ignored machine-readable artifact.

This is a local policy decision, not a universal claim about Cargo diagnostics.
A different project or error-heavy build may produce different evidence and can
replace the local policy after its own benchmark run.
