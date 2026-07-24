# RTK-WAD dogfood cycles (2026-07-24)

Two additional read-only dogfood cycles ran against the real `rtk-wsl` and
Flowpeek worktrees using one isolated, cumulative local policy. Each cycle
executed Git status/log and two repository searches. All eight new invocations
exited successfully; together with the preceding verification cycle, the local
ledger contained twelve invocations.

| Route | Invocations | Measured token saving |
| --- | ---: | ---: |
| Raw Windows Git | 6 | 0 |
| Native RTK search | 6 | 35,370 |

The cumulative ledger reported 51,398 input tokens, 16,028 output tokens, and
35,370 saved tokens (68.8%). Route choice remained stable across both projects:
the local evidence chose raw for compact Git reads and native RTK for the
token-heavy searches.

This is dogfood evidence, not a universal performance claim. The isolated
ledger and policy are intentionally local and ignored by Git.

An additional isolated toolchain cycle used the generated five-sample
`go:test-all` policy on `go-practice`. `rtk-wad go test ./...` selected native
RTK, exited successfully, and reported the compact `Go test: No tests found`
result. The near-miss `go test` form remained raw, proving that the promotion
does not broaden beyond the benchmarked argv contract.
