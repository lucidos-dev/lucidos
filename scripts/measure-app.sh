#!/usr/bin/env bash
# Usage: scripts/measure-app.sh chromium|webkit
# Record three samples per app journey against a disposable e2e workspace.
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

case "${1:-}" in
    chromium | webkit) BROWSER="$1" ;;
    *) echo "Usage: $0 chromium|webkit" >&2; exit 2 ;;
esac
[ "$#" -eq 1 ] || exit 2

LAB_KIND=app
LAB_TOOLCHAIN="$(node --version)"
LAB_RUN="$(node -p 'require("node:crypto").randomUUID()')"
mkdir -p target
LAB_OUT="$(mktemp -d "$PROJECT_DIR/target/measure-app.XXXXXX")"
echo "App lab output: $LAB_OUT"

# shellcheck source=scripts/lib/lab_slot.sh
source "$SCRIPT_DIR/lib/lab_slot.sh"
lab_idle || {
    rc=$?
    if [ "$rc" -eq 75 ]; then lab_defer setup; fi
    exit "$rc"
}
[ -x node_modules/.bin/esbuild ] || { echo "Workspace dependencies are required." >&2; exit 2; }

caller_workspace="${LUCIDOS_WORKSPACE:-}"
caller_private="${LUCIDOS_PRIVATE_DIR:-}"
unset E2E_WORKSPACE LUCIDOS_E2E_UMBRELLA NO_RESET LUCIDOS_E2E_ENGINE_BUILT
# shellcheck disable=SC2034 # e2e.sh reads the project to select its lock.
E2E_LOCK_PROJECTS="desktop-$BROWSER"
# shellcheck source=scripts/lib/e2e.sh
source "$SCRIPT_DIR/lib/e2e.sh"
# shellcheck source=scripts/lib/build_slot_lease.sh
source "$SCRIPT_DIR/lib/build_slot_lease.sh"
e2e_private="$LUCIDOS_PRIVATE_DIR"

setup_lab() {
    if [ "${LUCIDOS_BUILD_SLOT_GRANTED_CLASS:-}" != background ] ||
        [ -z "${LUCIDOS_BUILD_SLOT_REQUEST_ID:-}" ]; then
        echo "The app lab needs a broker that records background leases." >&2
        return 2
    fi
    LAB_SETUP_REQUEST="$LUCIDOS_BUILD_SLOT_REQUEST_ID"
    local -x LUCIDOS_PRIVATE_DIR="$e2e_private"
    setup_e2e_session app-lab
}

defer_busy_setup() {
    local status=$?
    if [ "$status" -eq "$E2E_LOCK_BUSY_EXIT" ]; then
        local -x LUCIDOS_PRIVATE_DIR="$caller_private"
        lab_defer setup
    fi
}

since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
# The e2e helper replaces this trap after it acquires a slot.
trap defer_busy_setup EXIT
LUCIDOS_PRIVATE_DIR="$caller_private" LUCIDOS_BUILD_SLOT_CLASS=background \
    build_slot_leased "app lab: setup" setup_lab

# Setup repoints the workspace for teardown. Keep the broker and result events in the caller's workspace.
(
    if [ -n "$caller_workspace" ]; then export LUCIDOS_WORKSPACE="$caller_workspace"; else unset LUCIDOS_WORKSPACE; fi
    if [ -n "$caller_private" ]; then export LUCIDOS_PRIVATE_DIR="$caller_private"; else unset LUCIDOS_PRIVATE_DIR; fi
    node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" verify-release "$LAB_SETUP_REQUEST" "$since"

    export LUCIDOS_EXTERNAL_PG_PORT="$PG_PORT"
    driver="$LAB_OUT/app-lab.mjs"
    fixture="$LAB_OUT/fixture.json"
    driver_command=(env "LUCIDOS_PRIVATE_DIR=$e2e_private" node "$driver")
    lab_step "app lab: prepare driver" node_modules/.bin/esbuild scripts/scoreboard/app-lab.ts \
        --bundle --platform=node --format=esm --packages=external --outfile="$driver"
    lab_step "app lab: seed" "${driver_command[@]}" seed "$fixture"
    samples=()
    sample_count="$(node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" sample-count)"
    for journey in cold-boot notification-deep-link thread-navigation; do
        for ((sample = 1; sample <= sample_count; sample++)); do
            sample_file="$LAB_OUT/$journey-$sample.json"
            lab_step "app lab: $journey" "${driver_command[@]}" measure "$BROWSER" "$journey" "$fixture" "$sample_file" "$sample"
            samples+=("$sample_file")
        done
    done
    node "$SCRIPT_DIR/scoreboard/lab-cli.mjs" record "$LAB_KIND" "$LAB_TOOLCHAIN" "$LAB_RUN" "$fixture" "${samples[@]}"
)
