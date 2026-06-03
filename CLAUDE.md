# CLAUDE.md

Top-level guidance for Claude Code. **Terms** (intent, knowhow, app, trigger, sub-thread, aggregate, EventBus, ThreadEvent, …) are defined in [`system-knowhow/glossary.md`](system-knowhow/glossary.md) (user-facing, base layer) and [`docs/glossary.md`](docs/glossary.md) (dev-only, extends the user-facing one). Use the canonical word — never a synonym. The glossary is a **living artifact**: grilling, brainstorming, and design dialogue actively use it (phrase questions in canonical terms, flag synonym reaches) and actively sharpen it (propose new entries / refinements in-conversation when a concept crystallizes). See `.claude/rules/glossary.md` § "Active use during design dialogue".

Detailed conventions live in `.claude/rules/` files (each has glob frontmatter — Claude reads them when working on matching files).

- `glossary.md` — canonical-terms rule + active-use-and-sharpen rule for design dialogue. Use the word from `system-knowhow/glossary.md` or `docs/glossary.md`, not a synonym; new concept → add an entry in the same change. Loads on any markdown / Rust / TS edit.
- `rust.md` — Rust, events, migrations, EventBus, HTTP/API rules
- `db.md` — DB schema reference (tables, event types, column notes); loads on `*.sql` / `migrations/**`
- `frontend.md` — TS/CSS, `Loadable<T>`, intent-vs-logic, component conventions
- `testing.md` — Browser e2e (Playwright), API e2e, contract tests
- `scripts.md` — Dev/build scripts, ports, Makefile
- `system-knowhow.md` — drift-prevention rule: code changes that touch a documented surface (`ThreadEvent`, `SystemEvent`, scheduler blocklist, `lucidos.*` SDK, `lucidos` CLI, plugin manifest, `apis.json` shape, glossary entries, …) MUST update the matching `system-knowhow/*.md` or glossary in the same change; `/harden` flags omissions. Also covers recipe-maintenance reminders (workspace-audit / workspace-learning). Loads on `system-knowhow/**` and the documented engine surfaces it gates.
- `sdk.md` — JS SDK extension rules (check `js-sdk.md` first, update it in the same commit); loads on `packages/lucidos-sdk/**`

## Workspace Rules

- **Personal workspace** (`~/workspaces/personal`) is the user's live data. **Reads are fine** — `psql` SELECT, file/log reads, GET API calls, doc updates against personal data are all OK and often necessary for debugging. **Mutations are NOT** — never restart, rebuild, kill the engine, write/delete files in the workspace, or send state-changing API requests (POST/PUT/DELETE). Only exception: user explicitly says "restart the personal workspace" (confirm first). Use `~/workspaces/dev` for development.
- **Confirm before mutating.** State-changing actions require user confirmation unless part of an agreed plan. Read-only (GET, file/log reads, `git status`/`log`/`diff`) is always fine.
- **Browser**: headed Chrome, window `osascript -e 'tell application "Google Chrome" to set bounds of front window to {3840, 200, 5120, 2357}'`, zoom `document.body.style.zoom = '125%'`. Default workspace ports: read `~/workspaces/dev/.lucidos/ports`, navigate to `https://localhost:$VITE_PORT`.

## CC Operational Rules

- **Worktree isolation.** CC sessions run in isolated worktrees under `<workspace>/.lucidos/worktrees/`. All edits, builds, script runs stay in the worktree. Scripts resolve paths via `SCRIPT_DIR` — never reference main's absolute path. Before finishing: `git diff` and `git checkout -- <file>` to discard abandoned edits.
- **Never kill broadly.** Multiple workspaces (dev, personal, work, e2e-test) run concurrently. NEVER use `pkill`, `killall`, or broad `pgrep | xargs kill` on `lucidos-engine` — macOS `pkill` excludes ancestors, so the calling engine survives while silently killing every other workspace. To stop one: `./scripts/stop.sh -w <ws>` or `kill $(cat <ws>/.lucidos/engine.pid)`.
- **Never `pgrep` to wait.** CC sessions share the host process namespace, so `pgrep -f "cargo test ..."` matches *every* session's tests. Polling patterns like `while ps -p $(pgrep -f "cargo test") > /dev/null; do sleep 30; done` wedge until the Bash timeout fires. For long commands, use the Bash tool's `run_in_background: true` and poll with `TaskOutput` — per-session and reliable. (CC removed the older `BashOutput`/`KillBash` names; `TaskOutput` and `TaskStop` are the current ones.) If you must wait in shell, capture the PID directly: `cargo test ... & BG=$!; wait $BG`.
- **Never run two `cargo test` invocations against the same worktree in parallel.** The `lucidos-engine` crate is heavy — a foreground `cargo test` while a background `cargo test` is also compiling forks two parallel rustc trees on the same crate and OOM-kills the host (SIGKILL → exit 137; SIGTERM → exit 143). Run one `cargo test` at a time: either fully foreground, or fully `run_in_background: true` and then `TaskOutput` for the result. Never both. Narrow the filter to a *specific test module* (e.g. `engine::agent_session::lifecycle::tests::`) instead of a broad path prefix (e.g. `engine::agent_session::`) — narrower filters are still useful even with one invocation, because they cut runtime and blast radius if anything hangs.
- **Bash exit 137 / 143 is a hard failure, not "task done".** Exit 137 = SIGKILL (OOM-killer), 143 = SIGTERM (something asked the process to die). When ANY tool — `cargo test`, build, e2e, anything — returns 137 or 143, the work is incomplete: re-run with a narrower scope, free memory, kill the parallel invocation, then retry. Never treat the non-zero exit as confirmation that the validating step ran. Engine-side defense: an empty assistant Result on a non-shutdown, non-error, non-cancel turn surfaces as `ResponseFailed` (see the empty-text branch of `classify_result` in `agent_session/lifecycle.rs`) so the UI shows a red dot instead of a silent "completed" turn — but don't rely on the safety net; fix the OOM by not running tests in parallel in the first place.
- **`Edit` needs a fresh `Read`.** Always Read immediately before Edit — an earlier Read in the same session doesn't count once context drifts, a sub-task intervenes, or a watcher/linter touches the file. On `File has been modified since read`, re-Read and re-Edit; do NOT retry the same Edit verbatim — the on-disk content has changed and the old `old_string` may no longer match.
- **Prefer `Glob`/`Grep` tools over `ls`/`grep` with absolute paths.** A remembered `/abs/path/to/file.rs` goes stale the moment a refactor moves it — `grep -n 'pat' /abs/path/to/file.rs` then fails with "no such file or directory", while `Grep pattern: 'pat'` finds the new location automatically. Cheaper too — dedicated tools skip the shell hop and avoid spending Bash-output tokens on misses. Reach for `Glob`/`Grep` for any path you didn't just see this turn.
- **Anchor cargo at the worktree root — never `cd ..` then cargo.** Your default cwd is the worktree root; `cd ..` from there lands at `.lucidos/worktrees/`, which has no `Cargo.toml`, and `cargo check -p lucidos-engine` then errors "could not find Cargo.toml". Run cargo from the worktree root, or pass `--manifest-path Cargo.toml` (or `crates/<name>/Cargo.toml`) if you must invoke it from elsewhere.
- **Quote globs in Bash — zsh fails the whole command on no match.** The Bash tool runs under zsh, where unquoted `ls rust-toolchain*` exits non-zero with `zsh: no matches found: rust-toolchain*` if zero files match — bash would pass the literal through, zsh aborts. Quote it (`ls 'rust-toolchain*'`) or use `Glob pattern: 'rust-toolchain*'` for a real match. The error mentions `zsh` and looks like a tool problem, not a shell-syntax one.

## Code Style — Core Principles

- **Fix root causes, not symptoms.** Trace bugs to the originating layer; don't paper over upstream problems with downstream filters.
- **Make impossible states impossible.** Model state with enums, newtypes, controlled transitions. Where the platform can't model fully (DOM vs JS), reconcile + test.
- **KISS.** Resist special cases, flags, branches. Leave files cleaner than you found them.
- **Test-driven.** Write tests first. Bug fixes reproduce with a failing test first. Run relevant suites before claiming done.
- **Filtered Rust tests need `--lib`**: `cargo test -p lucidos-engine --lib "filter"`. Without `--lib`, runs zero tests silently. Verify `running N tests` line is > 0.
- **Multiple test name filters need `--`**: `cargo test -p lucidos-engine --lib -- name1 name2 …`. Without the `--` separator, clap treats every name after the first as an unknown positional and fails with `error: unexpected argument 'X' found`.
- **Integration tests for refactors.** Refactors that change data flow MUST have integration tests — unit tests miss wiring bugs.
- **Contract tests are sacred.** If you change `thread_lifecycle.rs`, regenerate: `cargo test -p lucidos-engine generate_typescript_file -- --ignored && cargo test -p lucidos-engine generate_cross_validation_fixture_file -- --ignored`. Never hand-edit `src/generated/`.
- **Zero warnings, errors, failing tests.** "Pre-existing" is never an excuse — if you see it, you own it.
- **Test selection by what changed:**
  - Rust (`.rs`, `Cargo.toml`, `Cargo.lock`, `.sql`) → `cargo check` + engine tests. **Run engine tests via `make test` (= `./scripts/test-engine.sh`), not bare `cargo test -p lucidos-engine`** — the integration tests need Postgres (`setup_test_db` reads `TEST_DATABASE_URL`); with no DB up, every DB-backed test panics on connect and reports hundreds of false failures. The script provisions a dedicated, disposable `lucidos-pg-test` container and exports the URL (see `.claude/rules/scripts.md`). For a narrow filter: `./scripts/test-engine.sh -- -- <name>`. If you touched the HTTP API surface, also run `./scripts/e2e-api.sh` (lives in the `lucidos-e2e` crate, runs against a booted e2e workspace).
  - TypeScript (`.ts`, `.tsx`) → `npx tsc --noEmit` + `cd crates/lucidos-app && npm test`
  - CSS-only / docs-only → no tests
  - Mixed Rust + TS → run both
- **Test locations** — Rust unit: inline `#[cfg(test)]`. TS unit: `*.test.ts` next to source. Browser e2e: `crates/lucidos-app/e2e/*.spec.ts` (`./scripts/e2e-browser.sh`). API e2e: `crates/lucidos-e2e/tests/` (`./scripts/e2e-api.sh`). Contract tests: `crates/lucidos-app/src/generated/`. See `testing.md`.
- **Hardening enforced at Apply time.** CC MUST run `/harden` once the work is complete and you're about to hand back to the user — no exceptions, even for docs-only, CSS-only, or comment-only changes. Don't harden mid-work; one `/harden` covering the whole finished batch of commits is enough. The skill itself decides what to test (auto-skipping phases when no relevant layer applies) and iterates from Phase 1 if anything fails. If the marker is `MISSING` when the user clicks Apply, Apply runs `/harden` synchronously and the user waits — so don't leave a finished batch un-hardened. Never tell the user you're "postponing", "deferring", or "skipping" `/harden` — there is no such option in the system. Either Phase 0 reports `ALREADY_HARDENED` (say so and stop) or you run it. Saying you'll postpone and then having Apply run it anyway is a confusing lie.
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

Lucidos is a cognitive OS: prompt as primary interface, events as single source of truth, versioned artifacts in Git, resumable workflows.

- Events are immutable, append-only, **named in past tense** (`MessageReceived`).
- Git is the artifact store but **never the authority** — events are.
- Events are checkpoints; on crash, reconstruct state from the last confirmed step.
- External edits are staged, not immediately authoritative.
- **Broadcast and subscribe — never direct coupling.** Backend: state changes go through EventBus; consumers (SSE, projections, side effects) subscribe. Frontend: state flows through signals + SSE; components react to signal changes, never reach into other components or store internals. If you import a consumer into a producer, you've broken the pattern.
- **System events are individual variants, never wrappers.** Each event type is its own `SystemEvent` enum variant. Grouping is provided by `aggregate()` ("trigger", "app", …), not by `event_type: String` discriminators.
- **Rust is the source of truth for event types.** `SystemEvent` + `#[serde(tag = "type", content = "data")]` produces the wire format. Frontend handles the same names — no translation layers, no meta-events, no "entity changed" wrappers.

## Environment Variables

- `LUCIDOS_WORKSPACE` — workspace dir (default `./workspace`)
- `DATABASE_URL` — Postgres connection string for the engine. **Never hardcode the URL or password into a `psql`/Python invocation** — the engine sets `PGUSER`/`PGPASSWORD`/`PGHOST`/`PGPORT`/`PGDATABASE` in every spawned subprocess (CC sessions, bash + python tools, scheduled scripts), so just run `psql -c '…'` bare. Putting the URL in argv leaks the password into the persisted `Bash` tool-call payload that the steps UI renders.
- `LUCIDOS_MODEL` — LLM model (default `claude-opus-4-8@default`). `[1m]` = 1M context. `gpt-*` = OpenAI.
- `VERTEX_PROJECT_ID` / `VERTEX_REGION` — GCP (default region `europe-west1`)
- `OPENAI_API_KEY` — for `gpt-*` models
- `LUCIDOS_EMBEDDING_MODEL` — embedding model (`bge-small-en-v1.5` or `multilingual-e5-small`, default `multilingual-e5-small`)
- `LUCIDOS_EXTRACTION_MODEL` — default model the memory extractor falls back to when no per-call override is passed and the `model_memory` preference is also empty (default `gemini-3-flash-preview`)

## Workspace Layout & Taxonomy

See `docs/taxonomy.md` for the full content taxonomy. Key points:

- `.lucidos/` — gitignored, ephemeral runtime/cache; can be rebuilt.
- `data/artifacts/` — git-tracked, NEVER auto-delete.
- `data/postgres/` — gitignored event store.
- **Intent** = what the user wants (stable). **Knowhow** = how to achieve it (evolves). **Script** = code invoked by either.
- **Ownership**: everything lives with its consumer (inside `apps/`, `triggers/`, `knowhow/`).
- **Survivability test**: "Does this survive if I delete the app?" → top-level. "Only makes sense for this app?" → inside the app.
- Never use generic names like `app.md` or `knowhow.md` — name by what they describe.
- **Manifest** = user-facing UI. **Knowhow / intents** = engine-facing LLM context.
- **Triggers — keep procedure out of intent.** A trigger's `run.intent` is what the user would say ("notify me when X happens"). Every imperative verb about *how* (hit, parse, scan, fall back, retry, emit) belongs in a knowhow file. The trigger thread looks up knowhow itself via `load_knowhow` at fire time — same as chat — so there is no per-trigger allow-list to configure on the trigger. The HTTP API takes one big string and won't stop you from dumping the recipe in — that's the trap. See `docs/taxonomy.md` § Triggers for the worked example.

## One-Click Install

Self-contained desktop app — no terminal, Docker, or dev tools required (macOS `.app`, Windows `.msi`, Linux AppImage). Bundled: PostgreSQL+pgvector, engine binary, static frontend. No OS-specific commands in the engine; engine connects to Postgres over TCP. `.lucidos/` rebuildable, `artifacts/` + Postgres = complete state.

## Maintaining This File

When a new convention or architectural decision is established, update this file or the appropriate `.claude/rules/` file in the same session. Recipe-maintenance rules for `system-knowhow/` live in `.claude/rules/system-knowhow.md`.
