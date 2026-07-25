#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    printf '%s\n' 'usage: install.sh DESTINATION' >&2
    exit 2
fi

destination=$1
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fixture="$destination/rtk-fixture"
commands='aws curl docker gh glab kubectl oc psql wget'
mkdir -p "$destination"
cp "$script_dir/rtk-fixture" "$fixture"
chmod 755 "$fixture"
for command_name in $commands; do
    ln -sfn rtk-fixture "$destination/$command_name"
done
printf 'Installed Linux fixtures for %s\n' "$commands"
