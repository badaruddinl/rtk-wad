# Tokenizer bootstrap (P19)

`tiktoken==0.12.0` is a pinned WAD runtime dependency. A normal canonical WAD
installation provisions it in a private virtual environment before installing
the launcher.

On a machine without Python, inspect the exact plan first:

```powershell
.\scripts\install-tokenizer.ps1 -PlanPythonBootstrap
```

The only automatic candidate is the exact Windows Package Manager package:

```text
winget install --id Python.Python.3.12 --exact --source winget --accept-package-agreements --accept-source-agreements
```

Authorize it only as part of an intentional WAD installation:

```powershell
.\scripts\install.ps1 -InstallPython -ConfirmPythonInstall
```

The installer rejects an unconfirmed bootstrap, does not choose another package
manager, and does not activate `rtk-wad.exe` until both the private environment
and the pinned tokenizer import have been verified. A successful package-manager
operation whose executable is not visible yet requires a new terminal and a
rerun; it still leaves the launcher inactive.
