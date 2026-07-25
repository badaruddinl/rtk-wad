# P20 local freeze: 0.2.0-alpha.2

`0.2.0-alpha.2` is the local freeze candidate for the P18–P20 work plus the
canonical RTK-WAD artifact and honest local-accounting refinement. It is a new
prerelease baseline, not a replacement for the existing immutable `v0.1.0`
stable companion release. The published `v0.2.0-alpha.1` tag remains immutable
as the earlier P20 baseline.

## Included scope

- P18 exact-provider benchmark preflight and recorded matrix evidence;
- P19 bidirectional provider execution, generic diagnosis, and managed
  tokenizer dependency; and
- P20's strict no-publish local release gate.

## Current verification

The P20 gate passed on the freeze commit's immediate predecessor with the
exact Windows RTK, WSL1 RTK, and WSL2 RTK providers supplied explicitly. It
covered formatting, Clippy, 38 unit tests, 16 Windows/WSL process-contract
tests, release build, tokenizer contracts, installer/recovery, setup readiness,
the three-provider P18 manifest preflight, native command-manifest equality,
and package/archive hygiene.

The version-only freeze commit must run the same gate again before a tag is
created. The candidate intentionally performs no push, GitHub Release, binary
signing, or upstream contribution. Those actions require a separate release
decision after the versioned gate result is recorded.

## Release boundary

The candidate is suitable for focused dogfooding through the canonical
`rtk-wad` command. It is not a claim that every external toolchain is
performance-covered: benchmark coverage remains limited to the P18 artifacts
and documented fixture/toolchain evidence.
