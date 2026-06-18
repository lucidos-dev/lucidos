#!/usr/bin/env bash
# release_events.sh — emit ReleaseStep* / LucidosReleased domain events from the
# deterministic release pipeline (build-dmg.sh, release.sh, release-to-lucidos.sh).
#
# The deterministic shell pipeline is the SPINE: it emits its own events at each
# stage boundary so the Release Cockpit app (data/apps/release-cockpit/) lights up
# stage by stage. The cockpit is a PURE READ-ONLY CONSUMER of these events — it
# never emits them, and nothing here writes to the app.
#
# Cockpit contract (data/apps/release-cockpit/index.html):
#   - The cockpit keys each event on payload.step, which MUST be one of its STEP
#     ids:  creds  state  draft  push  build  codesign  notarize  upload  event
#   - It renders the note from `payload.detail || payload.summary`. We pass the
#     note via --summary, which the `lucidos` CLI injects into the payload as
#     `summary` — so the cockpit shows it. The note is therefore passed through
#     the flag (CLI-escaped), never embedded in --payload JSON.
#   - The embedded --payload JSON carries only safe literals (the fixed step id
#     and an N.N.N version), so no manual JSON escaping is required.
#   - LucidosReleased marks ALL steps done; it carries {version, commit, tag}.
#
# Emission is BEST-EFFORT. A release must never abort because the engine was
# briefly unreachable, so every emit is guarded and only warns on failure. The
# `lucidos` CLI is on PATH in every Lucidos-spawned subprocess; when it is absent
# (e.g. a manual run outside a Lucidos subprocess) these are silent no-ops.

# emit_release_step <Started|Succeeded|Failed> <step-id> <version> <summary>
emit_release_step() {
    local kind="$1" step="$2" version="$3" summary="$4"
    command -v lucidos >/dev/null 2>&1 || return 0
    if ! lucidos events emit "ReleaseStep${kind}" \
        --summary "$summary" \
        --payload "{\"step\":\"${step}\",\"version\":\"${version}\"}" \
        >/dev/null 2>&1; then
        echo "WARNING: failed to emit ReleaseStep${kind} for step '${step}' (cockpit may not update)" >&2
    fi
    return 0
}

# emit_lucidos_released <version> <commit> <tag> <summary>
emit_lucidos_released() {
    local version="$1" commit="$2" tag="$3" summary="$4"
    command -v lucidos >/dev/null 2>&1 || return 0
    if ! lucidos events emit LucidosReleased \
        --summary "$summary" \
        --payload "{\"version\":\"${version}\",\"commit\":\"${commit}\",\"tag\":\"${tag}\"}" \
        >/dev/null 2>&1; then
        echo "WARNING: failed to emit LucidosReleased (cockpit may not show completion)" >&2
    fi
    return 0
}
