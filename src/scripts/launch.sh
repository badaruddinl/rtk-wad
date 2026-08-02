lock_wait=$1
lock_path=$2
rtk_path=$3
cancel_nonce=$4
metrics_db_path=$5
extra_path=$6
ready_file=$7
attestation_file=$8
permit_file=$9
completion_file=${10}
launch_delay=${11}
completion_override=${12}
shift 12

if [ -z "$rtk_path" ]; then
    rtk_path="$HOME/.local/bin/rtk"
fi

user=${USER:-}
if [ -n "$extra_path" ]; then
    path_prefix="$extra_path:"
else
    path_prefix=""
fi
case "$cancel_nonce" in
    *[!0-9a-f]*|'') printf 'xuva: invalid cancellation nonce\n' >&2; exit 1 ;;
esac
if [ "${#cancel_nonce}" -ne 32 ]; then
    printf 'xuva: invalid cancellation nonce length\n' >&2
    exit 1
fi
if [ -z "$completion_file" ]; then
    printf 'xuva: missing completion attestation path\n' >&2
    exit 1
fi
completion_staging="${completion_file}.staging"
cancel_token=
group_has_other_members() {
    for stat_path in /proc/[0-9]*/stat; do
        [ -r "$stat_path" ] || continue
        process_id=${stat_path#/proc/}
        process_id=${process_id%/stat}
        [ "$process_id" != "$$" ] || continue
        stat_value=$(/bin/cat -- "$stat_path" 2>/dev/null) || continue
        # /proc/<pid>/stat wraps comm in parentheses, but comm itself may
        # contain ") ". Strip through the final delimiter before state.
        stat_fields=${stat_value##*) }
        set -- $stat_fields
        [ "${3:-}" = "$$" ] && return 0
    done
    return 1
}
signal_other_members() {
    member_signal=$1
    for stat_path in /proc/[0-9]*/stat; do
        [ -r "$stat_path" ] || continue
        process_id=${stat_path#/proc/}
        process_id=${process_id%/stat}
        [ "$process_id" != "$$" ] || continue
        stat_value=$(/bin/cat -- "$stat_path" 2>/dev/null) || continue
        stat_fields=${stat_value##*) }
        set -- $stat_fields
        [ "${3:-}" = "$$" ] || continue
        /bin/kill "-$member_signal" -- "$process_id" 2>/dev/null || true
    done
}
quiesce_process_group() {
    group_has_other_members || return 0
    signal_other_members TERM
    remaining=20
    while group_has_other_members && [ "$remaining" -gt 0 ]; do
        remaining=$((remaining - 1))
        /bin/sleep 0.05
    done
    group_has_other_members || return 0
    signal_other_members KILL
    remaining=20
    while group_has_other_members && [ "$remaining" -gt 0 ]; do
        remaining=$((remaining - 1))
        /bin/sleep 0.05
    done
    ! group_has_other_members
}
cleanup() {
    [ -z "$cancel_token" ] || /bin/rm -f -- "$cancel_token"
}
publish_completion() {
    attested_status=$1
    if [ -n "$completion_override" ]; then
        attested_status=$completion_override
    fi
    printf '%s:%s' "$cancel_nonce" "$attested_status" > "$completion_staging" &&
        /bin/mv -f -- "$completion_staging" "$completion_file"
}
finish() {
    completion_status=$?
    trap - EXIT INT TERM
    if ! quiesce_process_group; then
        printf 'xuva: child process group did not quiesce after command completion\n' >&2
        # Token removal plus any completion record is accepted by the Windows
        # parent as cleanup proof. Preserve the token and withhold completion
        # when a member survives so the parent must remain fail-closed.
        /bin/rm -f -- "$completion_staging"
        exit 125
    fi
    if publish_completion "$completion_status"; then
        # Never remove the process-group identity before the durable
        # completion proof exists. If publication fails, retain the token so
        # the parent can still prove that its exact group is gone.
        cleanup
    else
        /bin/rm -f -- "$completion_staging"
        printf 'xuva: unable to publish completion attestation\n' >&2
        completion_status=125
    fi
    exit "$completion_status"
}
trap finish EXIT
# The launcher remains the process-group leader. Let the foreground target
# receive escalation without allowing the supervising shell to discard its
# cancellation identity before all group members have stopped.
trap ':' INT TERM
case "$launch_delay" in
    0|1|2|3|4|5|6|7|8|9|10|11|12|13|14|15) ;;
    *) printf 'xuva: invalid test launch delay\n' >&2; exit 1 ;;
esac
case "$completion_override" in
    ''|0|1|2|3|4|5|6|7|8|9|[1-9][0-9]|1[0-9][0-9]|2[0-4][0-9]|25[0-5]) ;;
    *) printf 'xuva: invalid test completion override\n' >&2; exit 1 ;;
esac
if [ "$launch_delay" -ne 0 ]; then
    /bin/sleep "$launch_delay"
fi
uid=$(/usr/bin/id -u) || exit 1
runtime_root="/tmp/xuva-runtime-$uid"
umask 077
if [ -L "$runtime_root" ] || { [ -e "$runtime_root" ] && { [ ! -d "$runtime_root" ] || [ ! -O "$runtime_root" ]; }; }; then
    printf 'xuva: unsafe per-user runtime directory %s\n' "$runtime_root" >&2
    exit 1
fi
/bin/mkdir -m 0700 -p "$runtime_root" || exit 1
/bin/chmod 0700 "$runtime_root" || exit 1
if [ "$(/usr/bin/stat -Lc '%u:%a' -- "$runtime_root" 2>/dev/null)" != "$uid:700" ]; then
    printf 'xuva: invalid per-user runtime directory ownership or mode\n' >&2
    exit 1
fi
for stale_token in "$runtime_root"/cancel-*.pid; do
    [ -f "$stale_token" ] && [ ! -L "$stale_token" ] && [ -O "$stale_token" ] || continue
    [ "$(/usr/bin/stat -Lc '%u:%a' -- "$stale_token" 2>/dev/null)" = "$uid:600" ] || continue
    stale_worker=$(/bin/cat -- "$stale_token" 2>/dev/null) || continue
    case "$stale_worker" in
        *[!0-9]*|'') continue ;;
    esac
    if ! /bin/kill -0 -- "-$stale_worker" 2>/dev/null; then
        /bin/rm -f -- "$stale_token"
    fi
done
cancel_token="$runtime_root/cancel-$cancel_nonce.pid"
if [ -e "$cancel_token" ] || [ -L "$cancel_token" ] ||
   ! (set -C; printf '%s' "$$" > "$cancel_token"); then
    printf 'xuva: unable to create a private cancellation token\n' >&2
    exit 1
fi
/bin/chmod 0600 "$cancel_token" || { /bin/rm -f -- "$cancel_token"; exit 1; }
attestation_staging="${attestation_file}.staging"
if [ -z "$attestation_file" ] || [ -z "$permit_file" ] ||
   ! printf '%s' "$cancel_nonce" > "$attestation_staging" ||
   ! /bin/mv -f -- "$attestation_staging" "$attestation_file"; then
    /bin/rm -f -- "$attestation_staging"
    printf 'xuva: unable to attest the private cancellation token\n' >&2
    exit 1
fi
remaining=500
while [ ! -r "$permit_file" ]; do
    if [ "$remaining" -le 0 ]; then
        printf 'xuva: parent did not authorize the cancellation-ready launch\n' >&2
        exit 1
    fi
    remaining=$((remaining - 1))
    /bin/sleep 0.02
done
permit_nonce=$(/bin/cat -- "$permit_file") || exit 1
if [ "$permit_nonce" != "$cancel_nonce" ]; then
    printf 'xuva: launch permit does not match the cancellation token\n' >&2
    exit 1
fi
if [ "$lock_path" = "/tmp/xuva.lock" ]; then
    lock_root="/tmp/xuva-lock-$(/usr/bin/id -u)"
    if [ -L "$lock_root" ] || { [ -e "$lock_root" ] && { [ ! -d "$lock_root" ] || [ ! -O "$lock_root" ]; }; }; then
        printf 'xuva: unsafe per-user lock directory %s\n' "$lock_root" >&2
        exit 1
    fi
    /bin/mkdir -m 0700 -p "$lock_root" || exit 1
    /bin/chmod 0700 "$lock_root" || exit 1
    lock_path="$lock_root/dispatcher.lock"
fi
exec 9>"$lock_path"
remaining=$((lock_wait * 10))
while ! /usr/bin/flock -n 9; do
    if [ "$remaining" -le 0 ]; then
        printf 'xuva: timed out waiting for lock %s\n' "$lock_path" >&2
        exit 1
    fi
    remaining=$((remaining - 1))
    /bin/sleep 0.1
done
if [ -n "$ready_file" ]; then
    printf 'ready' > "$ready_file"
fi
/usr/bin/env -i \
    HOME="$HOME" \
    USER="$user" \
    PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    RTK_DB_PATH="$metrics_db_path" \
    "$rtk_path" "$@"
status=$?
exit "$status"
