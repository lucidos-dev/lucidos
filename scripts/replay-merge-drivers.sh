#!/usr/bin/env bash
# Replay past merges of main into agent branches, once without the merge
# drivers and once through them, and report what the drivers change:
#
#   resolved   a conflict the drivers merged. `match` means the result is
#              the file the agent committed; `order` means the same lines in
#              another order; `differs` needs a human look.
#   still      a conflict the drivers leave (a path without a driver, or a
#              true same-entry conflict).
#   new        a path that merged cleanly without the drivers but not with them.
#
# Read-only: `git merge-tree` writes objects, never a ref, index or file.
# Uses this checkout's .gitattributes, so run it from a tree that has the
# attribute lines under test. Why: ADR 0460.
#
# Usage: ./scripts/replay-merge-drivers.sh [--since "21 days ago"] [--jobs 6]
#        [--cli path-to-lucidos]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/merge_drivers.sh
source "$SCRIPT_DIR/lib/merge_drivers.sh"

# merge_tree <text|drivers> <ours> <theirs>: merge-tree with every lucidos
# driver set to the merge it gets without the driver, or to the real driver.
# Setting both modes keeps a driver registered in this repo out of the baseline.
merge_tree() {
  local mode=$1 i=0 kind value
  local -a env=()
  for kind in "${MERGE_DRIVER_KINDS[@]}"; do
    if [ "$mode" = text ]; then
      value=$(merge_driver_plain_merge "$kind" %A %O %B %L)
    else
      value=$(merge_driver_command "$REPLAY_CLI" "$kind")
    fi
    env+=("GIT_CONFIG_KEY_$i=merge.lucidos-$kind.driver" "GIT_CONFIG_VALUE_$i=$value")
    i=$((i + 1))
  done
  env "GIT_CONFIG_COUNT=$i" "${env[@]}" git merge-tree --write-tree --name-only --no-messages "$2" "$3"
}

if [ "${1:-}" = --one ]; then
  merge=$2 ours=$3 theirs=$4
  text=$(merge_tree text "$ours" "$theirs") && text_rc=0 || text_rc=$?
  drivers=$(merge_tree drivers "$ours" "$theirs") && drv_rc=0 || drv_rc=$?
  if [ "$text_rc" -gt 1 ] || [ "$drv_rc" -gt 1 ]; then
    echo "error $merge"
    exit 0
  fi
  tree=$(head -1 <<<"$drivers")
  before=$(tail -n +2 <<<"$text")
  after=$(tail -n +2 <<<"$drivers")
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    if grep -qxF "$f" <<<"$after"; then
      echo "still $merge $f"
    elif [ "$(git rev-parse -q --verify "$tree:$f" || true)" = "$(git rev-parse -q --verify "$merge:$f" || true)" ]; then
      echo "resolved-match $merge $f"
    elif [ "$(git cat-file -p "$tree:$f" 2>/dev/null | sort)" = "$(git cat-file -p "$merge:$f" 2>/dev/null | sort)" ]; then
      echo "resolved-order $merge $f"
    else
      echo "resolved-differs $merge $f"
    fi
  done <<<"$before"
  while IFS= read -r f; do
    if [ -n "$f" ] && ! grep -qxF "$f" <<<"$before"; then
      echo "new $merge $f"
    fi
  done <<<"$after"
  exit 0
fi

since="21 days ago" jobs=6 cli_arg=""
while [ $# -gt 0 ]; do
  case $1 in
    --since) since=$2; shift 2 ;;
    --jobs) jobs=$2; shift 2 ;;
    --cli) cli_arg=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
REPLAY_CLI=$(merge_driver_cli "$cli_arg")
export REPLAY_CLI

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git log main --since="$since" --merges --format='%H %P %s' |
  awk '/Merge branch .main. into/ {print $1, $2, $3}' >"$work/merges"
echo "replaying $(wc -l <"$work/merges" | tr -d ' ') merges of main into a branch since $since"
xargs -P "$jobs" -L 1 "$0" --one <"$work/merges" >"$work/results"

awk '
  { kind[$1]++; merges[$2] = 1 }
  $1 == "still" || $1 == "new" || $1 == "error" { stuck[$2] = 1 }
  $1 ~ /^resolved/ { touched[$2] = 1 }
  END {
    for (m in touched) if (!(m in stuck)) cleared++
    printf "merges with a text conflict, or a new one: %d\n", length(merges)
    printf "merges the drivers make clean: %d\n", cleared
    printf "files resolved, same as the agent: %d\n", kind["resolved-match"]
    printf "files resolved, same lines, other order: %d\n", kind["resolved-order"]
    printf "files resolved, differs from the agent: %d\n", kind["resolved-differs"]
    printf "files still conflicted: %d\n", kind["still"]
    printf "files newly conflicted: %d\n", kind["new"]
    printf "merges git could not replay: %d\n", kind["error"]
  }' "$work/results"
echo
echo "per file with a driver (count, outcome, path):"
awk '{print $3}' "$work/results" | sort -u |
  git check-attr --stdin merge | awk -F': ' '$3 ~ /^lucidos-/ {print $1}' >"$work/driven"
awk 'NR == FNR {driven[$1] = 1; next} ($3 in driven) {print $1, $3}' "$work/driven" "$work/results" |
  sort | uniq -c | sort -k3,3 -k1,1nr
echo
echo "merges whose driver result differs from the agent's, for review:"
grep -E '^resolved-differs' "$work/results" | sort -k3,3 || true
