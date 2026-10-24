# shellcheck shell=bash
# The git merge drivers this repository registers: `lucidos merge-driver`,
# one per kind, named by `.gitattributes` as `merge=lucidos-<kind>`.
# `scripts/merge-drivers.sh` registers them and `scripts/replay-merge-drivers.sh`
# replays history through them, both from these definitions. Why: ADR 0460.

# shellcheck disable=SC2034 # read by the two scripts that source this file
MERGE_DRIVER_KINDS=(sorted-lines blocks)

# merge_driver_plain_merge <kind> <ours> <base> <theirs> <marker-size>
#
# The merge a path gets when its driver does not run: git's text merge, or
# union for the ADR index, which is what it had before (ADR 0040).
merge_driver_plain_merge() {
  local union=""
  [ "$1" = sorted-lines ] && union="--union "
  printf 'git merge-file %s-L ours -L base -L theirs --marker-size %s %s %s %s' \
    "$union" "$5" "$2" "$3" "$4"
}

# merge_driver_command <absolute-cli-path> <kind>
#
# The command git runs for each path. The driver's own exit 0 or 1 is the
# merge. Any other status means it did not merge (a missing binary, a CLI that
# predates the subcommand, a crash), and the ours file is untouched. The path
# then gets merge_driver_plain_merge. Without that, git would keep the
# untouched ours file and silently drop the other side.
merge_driver_command() {
  local cli=$1 kind=$2 plain
  # shellcheck disable=SC2016 # the driver's own positional args, expanded by its sh
  plain=$(merge_driver_plain_merge "$kind" '"$2"' '"$1"' '"$3"' '"$4"')
  printf '%s' "sh -c 'cli=\$1 kind=\$2; shift 2; \"\$cli\" merge-driver \"\$kind\" \"\$@\"; s=\$?; [ \"\$s\" -le 1 ] && exit \"\$s\"; echo \"lucidos merge-driver did not merge (\$s); merging without it\" >&2; exec $plain' lucidos-merge-driver '$cli' $kind \"%O\" \"%A\" \"%B\" %L"
}

# merge_driver_cli [candidate]
#
# Print the absolute path of a `lucidos` CLI that has the merge-driver
# subcommand, from the candidate or PATH. Fails with a reason on stderr.
merge_driver_cli() {
  local cli=${1:-$(command -v lucidos || true)}
  if [ -z "$cli" ]; then
    echo "no lucidos CLI on PATH" >&2
    return 1
  fi
  case $cli in
    /*) ;;
    *) cli="$(cd "$(dirname "$cli")" && pwd)/$(basename "$cli")" ;;
  esac
  case $cli in
    *"'"*)
      echo "refusing a CLI path with a single quote: $cli" >&2
      return 1
      ;;
  esac
  if ! "$cli" merge-driver --help >/dev/null 2>&1; then
    echo "$cli has no merge-driver subcommand yet (it predates it)" >&2
    return 1
  fi
  printf '%s\n' "$cli"
}
