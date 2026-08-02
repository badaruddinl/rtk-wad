metrics_db_path=$1
extra_path=$2
ready_file=$3
attestation_file=$4
permit_file=$5
completion_file=$6
attestation_delay=$7
marker_validator=$8
completion_override=$9
marker_path=${10}
fixed_executable=${11}
expected_file_key=${12}
expected_size=${13}
expected_modified_stamp=${14}
shift 14

case "$attestation_delay" in
    0|1|2|3|4|5) ;;
    *) printf 'xuva: invalid test attestation delay\n' >&2; exit 126 ;;
esac
case "$completion_override" in
    ''|0|1|2|3|4|5|6|7|8|9|[1-9][0-9]|1[0-9][0-9]|2[0-4][0-9]|25[0-5]) ;;
    *) printf 'xuva: invalid test completion override\n' >&2; exit 126 ;;
esac
if [ -z "$attestation_file" ] || [ -z "$permit_file" ] ||
   [ -z "$completion_file" ] || [ -z "$marker_validator" ]; then
    printf 'xuva: incomplete WSL1 supervision metadata\n' >&2
    exit 126
fi
if [ "$attestation_delay" -ne 0 ]; then
    /bin/sleep "$attestation_delay"
fi
installation_id=$(/bin/sh -c "$marker_validator" xuva-marker-validator "$marker_path") || {
    marker_description=${marker_path:-/etc/xuva-dedicated-wsl1}
    printf 'xuva: WSL1 distro lacks a valid dedicated-runtime marker at %s\n' "$marker_description" >&2
    exit 126
}
attestation_staging="${attestation_file}.staging"
completion_staging="${completion_file}.staging"
if ! printf '%s' "$installation_id" > "$attestation_staging" ||
   ! /bin/mv -f -- "$attestation_staging" "$attestation_file"; then
    /bin/rm -f -- "$attestation_staging"
    printf 'xuva: unable to attest the dedicated WSL1 runtime\n' >&2
    exit 126
fi
remaining=500
while [ ! -r "$permit_file" ]; do
    if [ "$remaining" -le 0 ]; then
        printf 'xuva: parent did not authorize the attested WSL1 launch\n' >&2
        exit 126
    fi
    remaining=$((remaining - 1))
    /bin/sleep 0.02
done
permit_id=$(/bin/cat -- "$permit_file") || exit 126
if [ "$permit_id" != "$installation_id" ]; then
    printf 'xuva: WSL1 launch permit does not match the dedicated runtime\n' >&2
    exit 126
fi

group_has_other_members() {
    for stat_path in /proc/[0-9]*/stat; do
        [ -r "$stat_path" ] || continue
        process_id=${stat_path#/proc/}
        process_id=${process_id%/stat}
        [ "$process_id" != "$$" ] || continue
        stat_value=$(/bin/cat -- "$stat_path" 2>/dev/null) || continue
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
publish_completion() {
    attested_status=$1
    if [ -n "$completion_override" ]; then
        attested_status=$completion_override
    fi
    printf '%s:%s' "$installation_id" "$attested_status" > "$completion_staging" &&
        /bin/mv -f -- "$completion_staging" "$completion_file"
}
finish() {
    completion_status=$?
    trap - EXIT INT TERM
    if ! quiesce_process_group; then
        /bin/rm -f -- "$completion_staging"
        printf 'xuva: WSL1 child process group did not quiesce\n' >&2
        exit 125
    fi
    publish_completion "$completion_status" || {
        /bin/rm -f -- "$completion_staging"
        printf 'xuva: unable to publish WSL1 completion attestation\n' >&2
        completion_status=125
    }
    exit "$completion_status"
}
trap finish EXIT
trap ':' INT TERM

user=${USER:-}
if [ -n "$extra_path" ]; then
    path_prefix="$extra_path:"
else
    path_prefix=""
fi
if [ "$fixed_executable" = "@default-rtk@" ]; then
    fixed_executable="$HOME/.local/bin/rtk"
fi
if [ -n "$expected_file_key" ] || [ -n "$expected_size" ] || [ -n "$expected_modified_stamp" ]; then
    if [ -z "$fixed_executable" ] || [ -z "$expected_file_key" ] ||
       [ -z "$expected_size" ] || [ -z "$expected_modified_stamp" ]; then
        printf 'xuva: incomplete provider executable identity\n' >&2
        exit 126
    fi
    actual_identity=$(/usr/bin/stat -Lc '%d:%i|%s|%y' -- "$fixed_executable" 2>/dev/null) || {
        printf 'xuva: provider executable disappeared before launch: %s\n' "$fixed_executable" >&2
        exit 126
    }
    expected_identity="$expected_file_key|$expected_size|$expected_modified_stamp"
    if [ "$actual_identity" != "$expected_identity" ]; then
        printf 'xuva: provider executable identity changed before launch: %s\n' "$fixed_executable" >&2
        exit 126
    fi
fi
if [ -n "$ready_file" ]; then
    printf 'ready' > "$ready_file"
fi
if [ -n "$expected_file_key" ]; then
    /usr/bin/env -i \
        HOME="$HOME" \
        USER="$user" \
        PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
        RTK_DB_PATH="$metrics_db_path" \
        "$@"
elif [ -n "$fixed_executable" ]; then
    /usr/bin/env -i \
        HOME="$HOME" \
        USER="$user" \
        PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
        RTK_DB_PATH="$metrics_db_path" \
        "$fixed_executable" "$@"
else
    /usr/bin/env -i \
        HOME="$HOME" \
        USER="$user" \
        PATH="${path_prefix}$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
        RTK_DB_PATH="$metrics_db_path" \
        "$@"
fi
status=$?
exit "$status"
