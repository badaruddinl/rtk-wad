# Runtime dependencies

## Rust build contract

The crate declares Rust `1.88.0` as its minimum supported Rust version and CI
compiles the locked dependency graph with that toolchain. Reproducible release
and primary CI builds use the exact Rust `1.97.1` toolchain declared in
`rust-toolchain.toml`, with `rustfmt` and `clippy`. Release dependency scanning
uses `cargo-audit 0.22.2`; SBOM generation uses `cargo-cyclonedx 0.5.9`.

These versions are reviewed changes, not floating `stable` aliases. An MSRV
change must update `Cargo.toml`, CI, tests, and this contract together.

## XUVA tokenizer

`xuva` has no tokenizer runtime dependency. [tiktoken on
PyPI](https://pypi.org/project/tiktoken/) is an optional, official benchmark
dependency used only for reproducible token accounting.
`requirements/xuva-tokenizer.txt` is the canonical package manifest: the
installer reads its one exact `tiktoken==<version>` declaration to select the
private environment, verify the import, and report its installed version. An
explicit benchmark installation selects the private XUVA-owned environment.

The package is not installed globally and is not bundled into the Rust binary.
The core launcher installs without Python. `scripts/install.ps1 -InstallTokenizer`
verifies the imported package version before it activates that explicit install.
A dependency upgrade is a deliberate benchmark/release change: update the pin,
review the upstream release and license, regenerate benchmark evidence, and run
the tokenizer and packaging contracts.

`tiktoken` is distributed under the MIT license; XUVA itself remains licensed
under Apache-2.0. See the upstream package metadata for the current license and
Python-runtime requirements.

## Fresh-machine bootstrap

The tokenizer installer prefers an existing Python 3.12 runtime and accepts an
existing Python 3.9+ runtime. If none is available, it exposes a plan for the
single exact `winget` package `Python.Python.3.12`. It runs that package-manager
command only when the caller passes `-InstallTokenizer`, `-InstallPython`, and
`-ConfirmPythonInstall`; otherwise the core XUVA launcher is installed without
Python. It never
selects a secondary package manager or modifies a global Python environment.
