# Local metrics privacy

XUVA metrics are disabled unless `XUVA_METRICS=on` or `XUVA_METRICS=local` is
set explicitly. Metrics remain local; XUVA has no upload path for this data.
This opt-in ledger is independent from default-local adaptive calibration.

## Stored data

The durable ledger stores only:

- timestamp;
- selected route;
- bounded command family (for example `git:status`);
- aggregate input, output, and avoided-token counts reported by RTK;
- elapsed milliseconds and exit code;
- whether RTK produced a measurement.

The ledger retains the newest 10,000 invocation records.

## Data that is never persisted

XUVA does not persist command arguments, reconstructed command lines, project
paths, raw parser input, or parser error text. The compatibility database used
by RTK exposes redacted SQLite views and stores only numeric aggregates behind
those views.

Scratch databases and their `-wal` and `-shm` sidecars are removed by a Rust
`Drop` guard on success and error returns. A bounded stale-file sweep handles
artifacts left by forced process termination. On Windows, the state directory
and metrics files require persistent ACL support and grant access only to the
current user, `SYSTEM`, and local administrators. Metrics fail closed on a
Windows filesystem that cannot persist ACLs.

## User controls

```powershell
$env:XUVA_METRICS = "on"
xuva metrics status
xuva metrics purge
```

Unset `XUVA_METRICS` (or set it to `off`) to use the zero-ledger fast path.
`xuva metrics purge` removes only the metrics ledger, tracker template, and
scratch files; policy, calibration, provider, and setup state are preserved.

Eligible adaptive commands can still update bounded opaque calibration evidence
while metrics are off. They never create `metrics-v1.sqlite`; set
`XUVA_CALIBRATION=off` to disable calibration reads, temporary measurement, and
writes as a separate control.
