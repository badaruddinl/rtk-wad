#!/usr/bin/env sh
# Compatibility shim. New callers should invoke xuva-wsl.sh and set
# XUVA_WINDOWS_EXE. RTK_WAD_WINDOWS_EXE remains supported unchanged.
set -eu

: "${XUVA_WINDOWS_EXE:=${RTK_WAD_WINDOWS_EXE:-}}"
export XUVA_WINDOWS_EXE
exec "$(dirname "$0")/xuva-wsl.sh" "$@"
