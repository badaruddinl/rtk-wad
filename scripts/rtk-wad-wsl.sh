#!/usr/bin/env sh
# WSL-origin shim for the Windows RTK-WAD executable.
#
# WSL does not propagate arbitrary Linux environment variables to Windows
# processes. This shim sends WSL context and argv together as a base64
# NUL-delimited argument, so user arguments are never inserted into a shell
# command string.
set -eu

: "${RTK_WAD_WINDOWS_EXE:?set RTK_WAD_WINDOWS_EXE to the Windows rtk-wad.exe path or its /mnt path}"

distro=${WSL_DISTRO_NAME:-${RTK_WSL_DISTRO:-}}
if [ -z "$distro" ]; then
    printf '%s\n' 'rtk-wad-wsl: WSL_DISTRO_NAME is unavailable; set RTK_WSL_DISTRO explicitly' >&2
    exit 2
fi

cwd=$(pwd -P)
windows_cwd=$(wslpath -w -a "$cwd" 2>/dev/null || true)
case "$windows_cwd" in
    [A-Za-z]:\\*) ;;
    *) windows_cwd= ;;
esac
extra_path=${RTK_WSL_EXTRA_PATH:-}
output_adapter=${RTK_WAD_OUTPUT_ADAPTER:-auto}
payload=$(
    {
        printf '%s\0' 'v2' "$distro" "$cwd" "$windows_cwd" "$extra_path" "$output_adapter"
        printf '%s\0' "$@"
    } | base64 | tr -d '\n'
)

exec "$RTK_WAD_WINDOWS_EXE" "--wsl-bridge=$payload"
