#!/usr/bin/env bash
# Cargo nests RUSTC_WORKSPACE_WRAPPER inside RUSTC_WRAPPER. Apply the
# original compiler cache to dependencies. Time workspace rustc locally:
# passing this script to sccache confuses compiler detection, and timing
# the sccache client cannot measure the server's compiler RSS.
set -uo pipefail

if [ "${MEASURE_RUSTC_WORKSPACE:-}" != 1 ]; then
    if [ "$1" = "$0" ]; then
        export MEASURE_RUSTC_WORKSPACE=1
    elif [ "${1##*/}" = "clippy-driver" ]; then
        # cargo clippy supplies its own workspace wrapper.
        export MEASURE_RUSTC_WORKSPACE=1
        set -- "$0" "$@"
    elif [ -n "$MEASURE_RUSTC_CACHE" ]; then
        set -- "$MEASURE_RUSTC_CACHE" "$@"
    fi
    exec "$@"
fi

crate=''
target=''
is_test=0
args=("$@")
while [ "$#" -gt 0 ]; do
    case "$1" in
        --crate-name) crate="$2"; shift ;;
        --crate-type) target="$2"; shift ;;
        --test) is_test=1 ;;
    esac
    shift
done
if [ -z "${CARGO_PKG_NAME:-}" ] || [ -z "$crate" ] || [ "$crate" = ___ ]; then
    exec "${args[@]}"
fi

[ "$is_test" -eq 0 ] || target="test"
report="$MEASURE_RUSTC_DIR/$$"
time_flag=-v
[ "$(uname -s)" != Darwin ] || time_flag=-l
TIMEFORMAT="MEASURE_SECONDS %R"
{ time LC_ALL=C /usr/bin/time "$time_flag" -o "$report.time" "${args[@]}"; } 2> "$report.stderr"
status=$?
awk '
    /^MEASURE_SECONDS / { next }
    { print }
' "$report.stderr" >&2
seconds="$(awk '/^MEASURE_SECONDS / { print $2 }' "$report.stderr")"
rss="$(awk '
    /maximum resident set size/ { printf "%.0f", $1 / 1024 }
    /Maximum resident set size/ { print $NF }
' "$report.time")"
printf '%s\t%s\t%s\t%s\t%s\n' "$CARGO_PKG_NAME" "$crate" "${target:-bin}" "${seconds:-?}" "${rss:-?}" > "$report.tsv"
rm -f "$report.stderr" "$report.time"
exit "$status"
