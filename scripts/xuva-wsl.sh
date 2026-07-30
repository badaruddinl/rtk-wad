#!/usr/bin/env sh
# WSL-origin shim for the canonical Windows XUVA executable.
set -eu

: "${XUVA_WINDOWS_EXE:?set XUVA_WINDOWS_EXE to the Windows xuva.exe path or its /mnt path}"

distro=${WSL_DISTRO_NAME:-${XUVA_WSL_DISTRO:-}}
if [ -z "$distro" ]; then
    printf '%s\n' 'xuva-wsl: WSL_DISTRO_NAME is unavailable; set XUVA_WSL_DISTRO explicitly' >&2
    exit 2
fi

cwd=$(pwd -P)
windows_cwd=$(wslpath -w -a "$cwd" 2>/dev/null || true)
case "$windows_cwd" in
    [A-Za-z]:\\*|\\\\wsl.localhost\\*|\\\\wsl\$\\*) ;;
    *) windows_cwd= ;;
esac
extra_path=${XUVA_WSL_EXTRA_PATH:-}
output_adapter=${XUVA_OUTPUT_ADAPTER:-auto}
payload=$(
    {
        printf '%s\0' 'v2' "$distro" "$cwd" "$windows_cwd" "$extra_path" "$output_adapter"
        printf '%s\0' "$@"
    } | base64 | tr -d '\n'
)

exec "$XUVA_WINDOWS_EXE" "--wsl-bridge=$payload"
