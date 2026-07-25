# Runtime dependencies

## WAD tokenizer

`rtk-wad` manages [tiktoken on PyPI](https://pypi.org/project/tiktoken/) as a
first-class runtime dependency for reproducible token accounting. The exact
version is pinned in `requirements/wad-tokenizer.txt` and is installed into a
private WAD-owned virtual environment during a fresh canonical installation.

The package is not installed globally and is not bundled into the Rust binary.
The installer verifies the imported package version before it activates the
launcher. A dependency upgrade is a deliberate release change: update the pin,
review the upstream release and license, regenerate benchmark evidence, and run
the tokenizer and packaging contracts.

`tiktoken` is distributed under the MIT license; WAD itself remains licensed
under Apache-2.0. See the upstream package metadata for the current license and
Python-runtime requirements.
