# v0.2.0 release freeze

`v0.2.0` freezes the P18–P20 work plus the canonical RTK-WAD artifact, honest
local-accounting refinement, public product README, Windows CI coverage for the
repository's `master` integration path, and the canonical `rtk-wad` GitHub
repository name. It supersedes the earlier `v0.1.0` companion release as the
current stable baseline. The published `v0.2.0-alpha.1` and
`v0.2.0-alpha.2` tags remain immutable historical alpha baselines.

## Included scope

- P18 exact-provider benchmark preflight and recorded matrix evidence;
- P19 bidirectional provider execution, generic diagnosis, and managed
  tokenizer dependency; and
- P20's strict no-publish local release gate.
- A product-oriented README, original routing visual, and documentation index;
  and
- Windows CI triggers for pull requests to `master` as well as `development`.

## Required verification

The final gate must pass on this exact freeze commit with explicit Windows RTK,
WSL1 RTK, and WSL2 RTK providers. It covers formatting, Clippy, 38 unit tests,
16 Windows/WSL process-contract tests, release build, tokenizer contracts,
installer/recovery, setup readiness, the three-provider P18 manifest preflight,
native command-manifest equality, and package/archive hygiene.

The release process creates an annotated `v0.2.0` tag only after that result is
recorded. It does not create a GitHub Release, sign binaries, or submit upstream
contributions; those remain separate release decisions.

## Stable release boundary

The release is suitable for stable dogfooding through the canonical `rtk-wad`
command. It is not a claim that every external toolchain is performance-covered:
benchmark coverage remains limited to the P18 artifacts and documented
fixture/toolchain evidence.
