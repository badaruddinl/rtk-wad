# Adaptive decision hardening: P16

P16 keeps WAD's adaptive routing deterministic while preventing old local
measurements from silently selecting a route after the adapter context changes.
The safety classes from P15 remain the outer boundary: mutations, explicit
routes, foreign paths, and ambiguous command forms do not enter calibration or
policy-driven experimentation.

## Context-bound evidence

Both imported route policy and local calibration are bound to an opaque
16-character context signature derived from:

- the embedded upstream RTK manifest version;
- the configured native RTK path; and
- the effective Windows `PATH` value.

The signature is not a command log and does not contain arguments, output, or
the literal `PATH`. It makes a native RTK selection or a Windows tool-path
change invalidate old adaptive evidence before it can affect a route.

```powershell
xuva policy context
```

This prints the current policy schema, manifest version, and opaque context
signature. A benchmark artifact must use this exact context to be imported.

## Schema changes and migration

| State | P16 location | Behavior for older state |
| --- | --- | --- |
| Imported policy | `route-policy-v2.json` | v1 policy remains untouched and cannot select a P16 route. |
| Local calibration | `calibration-v2.json` | v1 calibration remains untouched and starts a fresh bounded cycle. |

The upgrade is non-destructive: P16 does not delete or rewrite prior local
files. New evidence is written atomically under the v2 paths.

Imported policy now requires all of the following before it can choose a route:

1. schema version 2;
2. the current RTK manifest version;
3. the current local context signature; and
4. at least five valid samples for the command key.

An import from another machine or another adapter installation is rejected with
an actionable diagnostic. An installed policy whose context later changes is
ignored and the manifest/static rule or local safe calibration resumes.

## Selection precedence

1. Explicit route and safety exclusions win.
2. A matching, validated imported policy wins.
3. A matching local calibration follows its bounded natural-invocation cycle.
4. The P15 command manifest supplies the conservative default route.

No layer retries a child after start. Raw routes retain no fabricated token
count; RTK routes use aggregate RTK metrics. P16 changes evidence validity, not
the underlying one-child execution contract.

## Verification

The process contract generates a matching v2 policy through `policy context`,
imports it, and proves it selects raw for a safe `rg` form. It then changes the
configured RTK path and proves the same policy is ignored, returning to the
manifest's native-RTK choice. Unit tests cover policy/context mismatch and
calibration-entry invalidation.
