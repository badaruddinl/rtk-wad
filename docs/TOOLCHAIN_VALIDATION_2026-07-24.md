# Toolchain route validation (2026-07-24)

`cargo` was validated on the RTK-WAD source worktree stored on `E:`. The
isolated WSL1 runtime intentionally has no Rust or Node toolchain, so routing
this command to WSL1 would be an avoidable failure. Stock Windows RTK accepts
the structured command `cargo check`, invokes the Windows Rust toolchain, and
completed successfully from the non-NTFS source worktree.

The validated invocation was:

```powershell
rtk-wad cargo check
```

It selected `native-rtk`, exited `0`, and produced compact cargo output. This
is a correctness validation, not a performance claim. A three-way toolchain
benchmark remains required before route decisions for other toolchain families
are promoted from the conservative WSL1 policy.
