nonce=$1
signal=$2
case "$nonce" in
    *[!0-9a-f]*|'') exit 2 ;;
esac
[ "${#nonce}" -eq 32 ] || exit 2
case "$signal" in
    INT|TERM|KILL) ;;
    *) exit 2 ;;
esac
uid=$(/usr/bin/id -u) || exit 3
runtime_root="/tmp/xuva-runtime-$uid"
if [ -L "$runtime_root" ] || [ ! -d "$runtime_root" ] || [ ! -O "$runtime_root" ]; then
    exit 3
fi
[ "$(/usr/bin/stat -Lc '%u:%a' -- "$runtime_root" 2>/dev/null)" = "$uid:700" ] || exit 3
cancel_token="$runtime_root/cancel-$nonce.pid"
if [ -L "$cancel_token" ] || [ ! -f "$cancel_token" ] || [ ! -O "$cancel_token" ]; then
    exit 4
fi
[ "$(/usr/bin/stat -Lc '%u:%a' -- "$cancel_token" 2>/dev/null)" = "$uid:600" ] || exit 4
worker=$(/bin/cat -- "$cancel_token") || exit 4
case "$worker" in
    *[!0-9]*|'') exit 4 ;;
esac
/bin/kill "-$signal" -- "-$worker"
