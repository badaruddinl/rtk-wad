# Cross-host provider resolution: PD2

PD2 extends the diagnostic-only Go provider resolver. It still does not change
normal command dispatch, install a toolchain, download packages, or request
elevation.

## Windows project with a WSL Go provider

A discovered WSL Go binary is no longer considered usable merely because it
exists. WAD invokes the fixed argument-vector form below for the selected
distro:

```text
wsl.exe -d <distro> --exec wslpath -a <Windows project path>
```

Only a successful absolute Linux result makes that WSL candidate usable. The
mapped project path appears as `project_path` in JSON and as a separate
candidate line in text output. This verifies the actual mount layout rather
than assuming `/mnt/<drive>`, so custom mounts can be supported when the distro
itself reports them.

`--exec` is mandatory. The legacy WSL command form can reinterpret backslashes
in Windows paths before `wslpath` receives them. PD2 preserves the Windows path
as one structured argument; tests cover spaces and literal shell characters.

## Other locality rules

- A WSL project is usable only with a provider in that exact distribution.
- A Windows provider remains intentionally unavailable for a WSL project.
- An unknown project location has no automatic provider.
- If both Windows and WSL providers are usable, the current diagnostic order
  prefers Windows. A later routing milestone will combine locality with RTK
  token evidence and measured latency.

The resulting recommendation remains informational in PD2. Existing
`rtk-wad go ...` routing is unchanged.
