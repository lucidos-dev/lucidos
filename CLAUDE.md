# CLAUDE.md

Top-level guidance for Claude Code. Detailed conventions live in `.claude/rules/` files (each has glob frontmatter — Claude reads them when working on matching files).

- `rust.md` — Rust, DB, events, migrations, EventBus, HTTP/API rules
- `frontend.md` — TS/CSS, `Loadable<T>`, intent-vs-logic, component conventions
- `testing.md` — Browser e2e (Playwright), API e2e, contract tests
- `scripts.md` — Dev/build scripts, ports, Makefile

## Workspace Rules

- **Personal workspace** (`~/workspaces/personal`) is the user's live data. NEVER touch it — no restart, rebuild, API request, or process kill. Only exception: user explicitly says "restart the personal workspace" (confirm first). Use `~/workspaces/dev` for development.
- **Confirm before mutating.** State-changing actions require user confirmation unless part of an agreed plan. Read-only (GET, file/log reads, `git status`/`log`/`diff`) is always fine.
- **Browser**: headed Chrome, window `osascript -e 'tell application "Google Chrome" to set bounds of front window to {3840, 200, 5120, 2357}'`, zoom `document.body.style.zoom = '125%'`. Default workspace ports: read `~/workspaces/dev/.cognos/ports`, navigate to `https://localhost:$VITE_PORT`.

## CC Worktree Isolation

CC sessions run in isolated worktrees under `<workspace>/.cognos/worktrees/`. All edits, builds, script runs stay in the worktree. Scripts resolve paths via `SCRIPT_DIR`. Never reference main's absolute path. Before finishing: `git diff` and `git checkout -- <file>` to discard abandoned edits.

## Process Safety — Never Kill Broadly

Multiple workspaces (dev, personal, work, e2e-test) run concurrently. NEVER use `pkill`, `killall`, or broad `pgrep | xargs kill` on `cognos-engine` — macOS `pkill` excludes ancestors, so the calling engine survives while silently killing every other workspace. To stop one: `./scripts/stop.sh -w <ws>` or `kill $(cat <ws>/.cognos/engine.pid)`.

## Long-Running Bash — Never `pgrep` to Wait

Multiple CC sessions share the host process namespace, so `pgrep -f "cargo test ..."` matches *every* session's tests, not just yours. Polling patterns like `while ps -p $(pgrep -f "cargo test") > /dev/null; do sleep 30; done` wedge until the Bash timeout fires (10 min). For long commands, use the Bash tool's `run_in_background: true` and poll with `BashOutput` — it's per-session and reliable. If you must wait in shell, capture the PID directly: `cargo test ... & BG=$!; wait $BG`.

## Code Style — Core Principles

- **Fix root causes, not symptoms.** Trace bugs to the originating layer; don't paper over upstream problems with downstream filters.
- **Make impossible states impossible.** Model state with enums, newtypes, controlled transitions. Where the platform can't model fully (DOM vs JS), reconcile + test.
- **KISS.** Resist special cases, flags, branches. Leave files cleaner than you found them.
- **Test-driven.** Write tests first. Bug fixes reproduce with a failing test first. Run relevant suites before claiming done.
- **Filtered Rust tests need `--lib`**: `cargo test -p cognos-engine --lib "filter"`. Without `--lib`, runs zero tests silently. Verify `running N tests` line is > 0.
- **Integration tests for refactors.** Refactors that change data flow MUST have integration tests — unit tests miss wiring bugs.
- **Contract tests are sacred.** If you change `thread_lifecycle.rs`, regenerate: `cargo test -p cognos-engine generate_typescript_file -- --ignored && cargo test -p cognos-engine generate_cross_validation_fixture_file -- --ignored`. Never hand-edit `src/generated/`.
- **Zero warnings, errors, failing tests.** "Pre-existing" is never an excuse — if you see it, you own it.
- **Test selection by what changed:**
  - Rust (`.rs`, `Cargo.toml`, `Cargo.lock`, `.sql`) → `cargo check` + `cargo test -p cognos-engine` (api_e2e tests are `#[ignore]`d — run separately via `./scripts/e2e-api.sh` if you touch HTTP API surface)
  - TypeScript (`.ts`, `.tsx`) → `npx tsc --noEmit` + `cd crates/cognos-app && npm test`
  - CSS-only / docs-only → no tests
  - Mixed Rust + TS → run both
- **Test locations** — Rust unit: inline `#[cfg(test)]`. TS unit: `*.test.ts` next to source. Browser e2e: `crates/cognos-app/e2e/*.spec.ts` (`./scripts/e2e-browser.sh`). API e2e: `crates/cognos-engine/tests/` (`./scripts/e2e-api.sh`). Contract tests: `crates/cognos-app/src/generated/`. See `testing.md`.
- **Hardening enforced at Apply time.** CC is asked to run `/harden` + the relevant test suites after implementation; the engine doesn't auto-trigger at idle. If the harden marker is missing when the user clicks Apply, Apply runs `/harden` + tests synchronously then proceeds. Marker existence (Fresh or Stale) means CC has hardened the branch at least once and is trusted — follow-up commits don't re-trigger.
- **Ruthless refactoring + DRY + no dead code.** Fix unclear names, dead branches, copy-paste when you touch a file. Delete unused — don't comment out or `_`-prefix.
- **No provider-specific instructions in code.** Use `web_search` for provider details.
- **Conventional commits**: `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`.

## Engine Statelessness

Engine must be restartable at any time without losing user-visible state. PostgreSQL + filesystem are sources of truth; in-memory is cache or active runtime.

- No critical state only in memory. Active processes are expected to die on restart.
- ALL running CC sessions must auto-resume after restart, even if the worktree has no git changes. The "already merged" check applies only to non-running sessions.
- Orphaned resources recover on startup. Frontend reconnects gracefully — page reload preserves visible UI state.
- **Allowed ephemeral**: channel senders/receivers, process handles, cancellation signals, `pending_captures`.
- **Not allowed**: preferences only in `RwLock` without DB write, pending changes only in memory.

## Core Architectural Principles

CognOS is a cognitive OS: prompt as primary interface, events as single source of truth, versioned artifacts in Git, resumable workflows.

- Events are immutable, append-only, **named in past tense** (`MessageReceived`).
- Git is the artifact store but **never the authority** — events are.
- Events are checkpoints; on crash, reconstruct state from the last confirmed step.
- External edits are staged, not immediately authoritative.
- **Broadcast and subscribe — never direct coupling.** Backend: state changes go through EventBus; consumers (SSE, projections, side effects) subscribe. Frontend: state flows through signals + SSE; components react to signal changes, never reach into other components or store internals. If you import a consumer into a producer, you've broken the pattern.
- **System events are individual variants, never wrappers.** Each event type is its own `SystemEvent` enum variant. Grouping is provided by `aggregate()` ("trigger", "app", …), not by `event_type: String` discriminators.
- **Rust is the source of truth for event types.** `SystemEvent` + `#[serde(tag = "type", content = "data")]` produces the wire format. Frontend handles the same names — no translation layers, no meta-events, no "entity changed" wrappers.

## Environment Variables

- `COGNOS_WORKSPACE` — workspace dir (default `./workspace`)
- `DATABASE_URL` — Postgres (default `postgres://cognos:cognos@localhost:5432/cognos`)
- `COGNOS_MODEL` — LLM model (default `claude-opus-4-7`). `[1m]` = 1M context. `gpt-*` = OpenAI.
- `VERTEX_PROJECT_ID` / `VERTEX_REGION` — GCP (default region `europe-west1`)
- `OPENAI_API_KEY` — for `gpt-*` models
- `COGNOS_EMBEDDING_MODEL` — embedding model (`bge-small-en-v1.5` or `multilingual-e5-small`, default `multilingual-e5-small`)

## Workspace Layout & Taxonomy

See `docs/taxonomy.md` for the full content taxonomy. Key points:

- `.cognos/` — gitignored, ephemeral runtime/cache; can be rebuilt.
- `data/artifacts/` — git-tracked, NEVER auto-delete.
- `data/postgres/` — gitignored event store.
- **Intent** = what the user wants (stable). **Knowhow** = how to achieve it (evolves). **Script** = code invoked by either.
- **Ownership**: everything lives with its consumer (inside `apps/`, `triggers/`, `knowhow/`).
- **Survivability test**: "Does this survive if I delete the app?" → top-level. "Only makes sense for this app?" → inside the app.
- Never use generic names like `app.md` or `knowhow.md` — name by what they describe.
- **Manifest** = user-facing UI. **Knowhow / intents** = engine-facing LLM context.

## One-Click Install

Self-contained desktop app — no terminal, Docker, or dev tools required (macOS `.app`, Windows `.msi`, Linux AppImage). Bundled: PostgreSQL+pgvector, engine binary, static frontend. No OS-specific commands in the engine; engine connects to Postgres over TCP. `.cognos/` rebuildable, `artifacts/` + Postgres = complete state.

## Maintaining This File

When a new convention or architectural decision is established, update this file or the appropriate `.claude/rules/` file in the same session.
