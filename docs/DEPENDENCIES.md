# Runtime dependencies

## WAD tokenizer

`rtk-wad` manages [tiktoken on PyPI](https://pypi.org/project/tiktoken/) as an
official first-class runtime dependency for reproducible token accounting.
`requirements/wad-tokenizer.txt` is the canonical package manifest: the
installer reads its one exact `tiktoken==<version>` declaration to select the
private environment, verify the import, and report its installed version. A
fresh canonical installation installs that declared package into a private
WAD-owned virtual environment.

The package is not installed globally and is not bundled into the Rust binary.
The installer verifies the imported package version before it activates the
launcher. A dependency upgrade is a deliberate release change: update the pin,
review the upstream release and license, regenerate benchmark evidence, and run
the tokenizer and packaging contracts.

`tiktoken` is distributed under the MIT license; WAD itself remains licensed
under Apache-2.0. See the upstream package metadata for the current license and
Python-runtime requirements.

## Fresh-machine bootstrap

The tokenizer installer prefers an existing Python 3.12 runtime and accepts an
existing Python 3.9+ runtime. If none is available, it exposes a plan for the
single exact `winget` package `Python.Python.3.12`. It runs that package-manager
command only when the caller passes both `-InstallPython` and
`-ConfirmPythonInstall`; otherwise the WAD launcher is not activated. It never
selects a secondary package manager or modifies a global Python environment.
