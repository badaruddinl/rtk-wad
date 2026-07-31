marker=${1:-/etc/xuva-dedicated-wsl1}
if [ -L "$marker" ] || [ ! -f "$marker" ] || [ ! -r "$marker" ] ||
   [ "$(/usr/bin/stat -Lc '%u:%a' -- "$marker" 2>/dev/null)" != "0:444" ] ||
   [ "$(/usr/bin/wc -l < "$marker" 2>/dev/null)" != "4" ] ||
   [ "$(/bin/grep -c '^product=xuva$' "$marker" 2>/dev/null)" != "1" ] ||
   [ "$(/bin/grep -c '^schema_version=1$' "$marker" 2>/dev/null)" != "1" ] ||
   [ "$(/bin/grep -c '^dedicated=true$' "$marker" 2>/dev/null)" != "1" ] ||
   [ "$(/bin/grep -c '^installation_id=' "$marker" 2>/dev/null)" != "1" ]; then
    exit 126
fi
installation_id=$(/bin/sed -n 's/^installation_id=//p' "$marker")
case "$installation_id" in
    ????????-????-????-????-????????????) ;;
    *) exit 126 ;;
esac
case "$installation_id" in
    *[!0-9A-Fa-f-]*) exit 126 ;;
esac
printf '%s\n' "$installation_id"
