#!/usr/bin/env bash
#
# eval-context-mode.sh - build the engine and the harness, then run the ADR 0110
# context-handling benchmark.
#
#   ./scripts/eval-context-mode.sh seed --repeat 1
#   ./scripts/eval-context-mode.sh run --config smoke --tasks T01,T02 --repeats 1
#   ./scripts/eval-context-mode.sh score --run-id <id>
#   ./scripts/eval-context-mode.sh analyse --run-id <id>
#   ./scripts/eval-context-mode.sh report --run-id <id>
#   ./scripts/eval-context-mode.sh replay --run-id <id> --list
#
# ONE CONFIGURATION AT A TIME (ADR 0110). `run` measures the `lean` arm alone
# unless `--arms lean,control` names both, and the report leads with absolute
# numbers rather than ratios. `--window <tokens>` declares a smaller context
# window on the seeded model row, which is the budget-pressure knob. A sweep is
# several runs at several windows, pooled by naming every id to `analyse` or
# `report`.
#
# THIS COSTS MONEY. A single-arm 14-task run is roughly $120 on Opus, and a
# four-window sweep is four of those. Nothing here runs from `make test`,
# `/harden` or a workflow, and `scripts/check-eval-not-a-test.sh` keeps it that
# way.
#
# EVERY ARM CAPTURES ITS OWN REQUESTS IN FULL. The arm engines boot with
# `LUCIDOS_EVAL_FULL_CAPTURE=1`, which lifts the two 8,000-char body caps on
# `ContextCaptured`, and the fixture seeds `capture_context`. That is what makes
# `replay` possible. It costs roughly 17 MB in the arm's own database, and it
# changes nothing for any other workspace.
#
# It never touches a live workspace. Every workspace it creates is named
# `eval-<label>-<arm>-<repeat>` under $LUCIDOS_EVAL_ROOT, and the harness
# refuses any path whose name lacks the `eval-` prefix (I5). Its database is
# `lucidos_` plus that same name.
#
# The label is what lets two providers run at once. An arm is a context-mode
# configuration and stays one, so the model is a separate axis and belongs in
# the name. Without it, two concurrent runs both want `eval-lean-1`.
#
# CONFIGURATION, all overridable from the environment.
#
#   LUCIDOS_EVAL_ROOT              where the arm workspaces are created
#   LUCIDOS_EVAL_PG_BASE           connection string WITHOUT a database name
#   LUCIDOS_EVAL_ENGINE_BIN        the engine binary under test
#   LUCIDOS_EVAL_MODEL             the pinned model (precondition P5)
#   LUCIDOS_EVAL_RUN_LABEL         names this run's workspaces. Defaults to the
#                                  model id, sanitised. Set it only to run the
#                                  SAME model twice at once, such as two
#                                  windows of a budget sweep started together
#   LUCIDOS_EVAL_MODEL_SET         every model the concurrent runs put under
#                                  test, comma separated. The judge is refused
#                                  if it names any of them. Defaults to this
#                                  run's own model
#   LUCIDOS_EVAL_MODEL_LABEL       its label in the seeded registry
#   LUCIDOS_EVAL_MODEL_PROVIDER    its provider in the seeded registry
#   LUCIDOS_EVAL_REASONING_EFFORT  the pinned reasoning effort
#   LUCIDOS_EMBEDDING_MODEL        the pinned embedding model (P5)
#   LUCIDOS_EVAL_READ_BY_ID        1 once query_events reads by id (P3)
#   LUCIDOS_EVAL_SKIP_BUILD        1 to reuse an already-built engine
#   LUCIDOS_EVAL_JUDGE_PROVIDER    pins the judge's provider, see below
#   LUCIDOS_EVAL_GATEWAY_URL       the gateway each arm registers with
#   LUCIDOS_EVAL_ENGINE_TLS_CERT   certificate the arm engines serve
#   LUCIDOS_EVAL_ENGINE_TLS_KEY    its key. Set both, or neither
#
# RUNNING TWO PROVIDERS AT ONCE. Give each run its own label, or let each take
# its model's, and their workspaces and databases never meet. Two things then
# have to be true of the shell you start them from.
#
#   * Every provider's key is exported at once. An arm's database is created
#     empty, so its `credentials` table has no row and each provider falls back
#     to its environment variable. The harness names no key itself, so both
#     arms inherit whatever the shell has.
#   * The context window is pinned with `--window`. Otherwise the engine infers
#     one per model, and gpt-5.6 infers far above Opus. The ceiling tasks then
#     measure two different budgets and report one number.
#
# THE JUDGE RUNS ON WHATEVER THIS MACHINE HAS. It calls its model through the
# engine's provider layer, so it reaches Vertex, Anthropic, OpenAI, OpenRouter,
# xAI and a local endpoint. Which one serves it, in order:
#
#   1. LUCIDOS_EVAL_JUDGE_PROVIDER, naming one of vertex, anthropic, openai,
#      openrouter, xai or local. A typo errors and names the six.
#   2. Otherwise the engine's own routing, from the judge model id. The judge
#      reads no workspace database, so a `claude-` id goes to Vertex and a
#      `gpt-` id to OpenAI.
#   3. When that provider is not configured here, and exactly one other is, the
#      judge uses that one and prints which.
#
# One cheap call then proves the pair before the first probe. A provider can
# hold a credential and still serve no such model, and that has to fail at the
# start rather than halfway down an append-only results file.
#
# The provider is NOT part of the fixture, deliberately. The judge model is
# hashed into fixture_hash because it is part of the measurement. The provider
# serving it is local credential setup, and hashing it would make one fixture
# fail to pool across two machines.
#
# The three TLS variables default off ONE detection, mirroring `detect_tls` in
# scripts/lib/workspace.sh, because the gateway decides its own scheme the same
# way. Override the URL alone and you can point at a gateway whose scheme the
# arms do not serve.
#
# EACH ARM IS REGISTERED AS A WORKSPACE, so the picker lists it and
# `/eval-lean-1/` is browsable during the run and after it. Registration is
# best-effort: no gateway, no local token or a refused connection logs a line
# and the run carries on. Autostart stays off, so the gateway never spawns an
# arm engine on its own boot. See the ADR 0087 amendment.
#
# THE TWO POSTGRES CLUSTERS HAVE TO BE ONE. During a run the gateway adopts the
# engine this harness booted, so it reads whatever LUCIDOS_EVAL_PG_BASE says.
# After the run it lazy-starts its own engine against ITS cluster's
# `lucidos_eval-lean-1`. Point this at the shared dev cluster, or a browsed arm
# opens empty once the run has ended.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Arms are ordinary workspaces. They live beside every other one, so the
# picker lists them without a nested root nobody else uses.
export LUCIDOS_EVAL_ROOT="${LUCIDOS_EVAL_ROOT:-$HOME/workspaces}"
export LUCIDOS_EVAL_PG_BASE="${LUCIDOS_EVAL_PG_BASE:-postgres://lucidos:lucidos@localhost:5435}"
export LUCIDOS_EVAL_MODEL="${LUCIDOS_EVAL_MODEL:-claude-opus-5@default}"
export LUCIDOS_EVAL_MODEL_LABEL="${LUCIDOS_EVAL_MODEL_LABEL:-Model under test}"
export LUCIDOS_EVAL_MODEL_PROVIDER="${LUCIDOS_EVAL_MODEL_PROVIDER:-vertex}"
export LUCIDOS_EVAL_REASONING_EFFORT="${LUCIDOS_EVAL_REASONING_EFFORT:-default}"

# The seeded memory vectors were produced by this model, and recall compares
# them against whatever the engine loads. A different one here would compare
# vectors from two models and quietly change what the control arm recalls.
export LUCIDOS_EMBEDDING_MODEL="${LUCIDOS_EMBEDDING_MODEL:-multilingual-e5-small}"

# Follow the dev stack's scheme, mirroring `detect_tls` in
# scripts/lib/workspace.sh arm for arm: the checkout's `.certs/` first, then the
# LUCIDOS_TLS_* pair, which is how a worktree supplies them because `.certs/` is
# gitignored. A gateway started with certificates probes and proxies https, so
# an arm serving plain http is unreachable through it. Both or neither: the
# harness refuses half a pair.
if [ -z "${LUCIDOS_EVAL_ENGINE_TLS_CERT:-}${LUCIDOS_EVAL_ENGINE_TLS_KEY:-}" ]; then
    if [ -f "$PROJECT_DIR/.certs/cert.pem" ] && [ -f "$PROJECT_DIR/.certs/key.pem" ]; then
        export LUCIDOS_EVAL_ENGINE_TLS_CERT="$PROJECT_DIR/.certs/cert.pem"
        export LUCIDOS_EVAL_ENGINE_TLS_KEY="$PROJECT_DIR/.certs/key.pem"
    elif [ -f "${LUCIDOS_TLS_CERT:-}" ] && [ -f "${LUCIDOS_TLS_KEY:-}" ]; then
        export LUCIDOS_EVAL_ENGINE_TLS_CERT="$LUCIDOS_TLS_CERT"
        export LUCIDOS_EVAL_ENGINE_TLS_KEY="$LUCIDOS_TLS_KEY"
    fi
fi

# The gateway serves whatever that SAME detection gave it, so the control URL
# has to agree. Hardcoding https against a checkout with no certificates fails
# every adoption and every port lookup, and the arms then go unregistered on
# every run with only a logged line to say so. Port per ADR 0014 §4.
if [ -n "${LUCIDOS_EVAL_ENGINE_TLS_CERT:-}" ]; then
    eval_gateway_proto=https
else
    eval_gateway_proto=http
fi
export LUCIDOS_EVAL_GATEWAY_URL="${LUCIDOS_EVAL_GATEWAY_URL:-$eval_gateway_proto://localhost:${LUCIDOS_DEV_GATEWAY_PORT:-5251}}"

if [ $# -eq 0 ]; then
    echo "usage: $0 <seed|run|score|analyse|report> [args...]" >&2
    exit 1
fi

mkdir -p "$LUCIDOS_EVAL_ROOT"

if [ -z "${LUCIDOS_EVAL_SKIP_BUILD:-}" ]; then
    echo "[eval] building the engine and the harness"
    cargo build --locked --release -p lucidos-engine -p lucidos-eval
fi

export LUCIDOS_EVAL_ENGINE_BIN="${LUCIDOS_EVAL_ENGINE_BIN:-$PROJECT_DIR/target/release/lucidos-engine}"

if [ ! -x "$LUCIDOS_EVAL_ENGINE_BIN" ]; then
    echo "ERROR: no engine binary at $LUCIDOS_EVAL_ENGINE_BIN" >&2
    exit 1
fi

echo "[eval] engine     $LUCIDOS_EVAL_ENGINE_BIN"
echo "[eval] workspaces $LUCIDOS_EVAL_ROOT"
echo "[eval] model      $LUCIDOS_EVAL_MODEL"

exec "$PROJECT_DIR/target/release/lucidos-eval" "$@"
