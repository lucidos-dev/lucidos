# CLAUDE.md

Working agreement for AI coding agents (Claude Code, Codex) and human contributors. **New here?** [`README.md`](README.md) covers prerequisites, dev setup, ports, and architecture: read it first, since this file is only the day-to-day conventions. **Terms** are defined in [`system-knowhow/glossary.md`](system-knowhow/glossary.md) (user-facing) and [`docs/glossary.md`](docs/glossary.md) (dev-only, extending it). Use the canonical word, never a synonym. `.claude/rules/glossary.md` carries that rule and the living-artifact practice; the `grill` skill carries the working method.

**Decision log.** Non-obvious decisions, especially deliberate *no*s and approaches we backed out of, live in [`docs/adr/`](docs/adr/README.md). Check there for the *why* before re-opening a settled question or proposing something the codebase pointedly avoids. When a design dialogue lands on a decision worth not re-litigating, add an entry in the same change.

**Create it with `./scripts/adr-new.sh <slug> "<one-line index text>"`, never by hand.** A number read off `main` collides with whatever a concurrent branch claimed, and the two differing filenames then merge *cleanly*, so the collision stays silent. The script allocates across `main`, every unmerged branch, and the working tree. [`docs/adr/index.md`](docs/adr/index.md) is `merge=union` so concurrent appends stop conflicting, and `./scripts/check-adrs.sh` enforces the result in `/harden` (`--fix` restores order).

**Implementation planning.** Complex work produces an *implementation plan* under `docs/plans/` before the first code edit, via the `implementation-plan` skill. Complex means any of: ADR- or design-thread-backed, cross-layer, a routing / topology / storage / security / migration / process change, or anything beyond a local bug fix. If decisions are still unsettled, use the `grill` skill first: `implementation-plan` consumes settled decisions and makes them executable.

A plan is not actionable until it lists load-bearing invariants, explicit non-goals, phase ordering, and a verification strategy per invariant. Those invariants stay in view while you edit, rather than first appearing at `/harden`. Enforcement is a durable *plan marker* gating the first edit and Apply ([`docs/glossary.md`](docs/glossary.md) § plan marker), and the engine states the procedure to every session.

Detailed conventions live in `.claude/rules/`. Six load in every session, and are already in front of you: `glossary.md`, `no-em-dashes.md`, `no-private-data.md`, `philosophy.md`, `prose.md`, `temporary-measures.md`. Ten load only when you read a matching file: `rust.md`, `db.md`, `frontend.md`, `frontend-css.md`, `testing.md`, `dev-runtime.md`, `build-release.md`, `front-door.md`, `system-knowhow.md`, `sdk.md`.

**[`docs/agent-config.md`](docs/agent-config.md) is where the mechanism is documented**: how scoping works, where a new instruction belongs (rule, skill, hook, or that page), and the context budget the whole set is gated on. Read it before adding or moving a rule. Three things bind here because they bind *before* any file is touched:

- **A path-conditional rule is not in context while you plan the change**, because it arrives only once you read a matching file. Anything that must be true before the first edit (a prohibition, a gate, a "never do X") belongs in this file or in an always-loaded rule, never in a `paths:` one. Rules also load on **read**, not on write, so a "when creating a file, always X" rule cannot be path-scoped at all.
- **A new script under `scripts/` matches no rule until you add its path.** `dev-runtime.md` and `build-release.md` list their scripts by name rather than taking a `scripts/**` catch-all, which is the only way an edit to a build script can skip the dev-runtime rule and vice versa. A rule that silently fails to load looks exactly like a rule that doesn't exist, so add the path in the same change that adds the script.
- **The always-loaded set is gated.** `./scripts/check-context-budget.sh` (in `/harden` Phase 4.5) fails if it grows past its ceiling, or if a rule that should be path-scoped silently became resident. Growing this file is not free: every byte is paid on every request of every session.

## Working in a Workspace

A **workspace** is a user's live Lucidos instance: a workspace directory with git-tracked artifacts plus one database in the shared Lucidos Postgres cluster. For development, run your own dedicated dev workspace (`./scripts/web-dev.sh -w ~/workspaces/<name> -b`; see [`README.md`](README.md) § Dev Setup). Multiple workspaces can run concurrently, each on its own engine port and database.

- **Reads are always fine; mutations need ownership and confirmation.** Reading a workspace (`psql` SELECT, file and log reads, GET calls) is safe, and often necessary for debugging. Mutating one requires that it is yours *and* that the action is part of an agreed plan: restart, rebuild, kill the engine, write or delete files, POST/PUT/DELETE. Otherwise confirm first. **Never mutate a workspace you don't own.**
- **Browser testing.** Point a headed browser at your dev workspace's user-facing URL. Each workspace records its assigned ports in `<workspace>/.lucidos/ports`; open `https://localhost:<vite-port>` (plain `http://` without local certs, see README § HTTPS for Local Development).
- **Local specifics live in [`WORKSPACES.md`](WORKSPACES.md).** Machine- and team-specific details live there: which named workspaces hold live data, local browser and test setup. It is internal-only, so put machine paths and personal setup there and never in this file. It also holds the **private-data denylist**, the exact tokens the release guard scans for. That block is the single sanctioned place for them (`.claude/rules/no-private-data.md`).

## Coding-Agent Operational Rules

- **Don't reinstall `node_modules`: the engine already provisioned it.** On spawn it hardlinks the main repo's installed trees into the worktree, for every Lucidos-source thread (`agent_session/run_session/spawn_context.rs`). It links the hoisted **worktree-root** tree **and** each workspace-member tree (today only `crates/lucidos-app/node_modules`), so `npx tsc --noEmit` and `npm test` both work. Never run `npm install`/`npm ci` yourself: it is redundant, and a bare `npm install` rewrites the committed lockfile (ADR 0020). External-repo and app-coding-agent threads are the only spawns the engine skips. Two traps when checking a tree is there:
  - **`vitest` lives in `crates/lucidos-app/node_modules`, NOT the root.** A root-only `ls node_modules/vitest` calls it missing when it is installed.
  - **The member tree carries no `.package-lock.json` marker**, since npm writes that only at the install root. Judge the root tree by `ls <root>/node_modules/.package-lock.json` and the member tree by `ls <root>/crates/lucidos-app/node_modules/vitest`.
- **Never kill broadly.** NEVER use `pkill`, `killall`, or a broad `pgrep | xargs kill` on `lucidos-engine`: concurrent workspaces share the process name, so a broad kill takes out every workspace but your own. Stop one with `./scripts/stop.sh -w <ws>` or `kill $(cat <ws>/.lucidos/engine.pid)`. (Deliberately also in the engine system prompt, which is the only surface reaching a session with no Lucidos checkout; `./scripts/check-prompt-mirror.sh` fails if either half goes missing. See `docs/agent-config.md` § The one sanctioned mirror.)
- **Running a `scripts/lib/*_test.sh` is in the same hazard class.** The port-allocator suite killed the user's live engine twice: it stubbed `port_is_free` but not `lsof`, so the reclaim path SIGUSR1'd the real engine. `ports.sh` now refuses to signal an ancestor of the running process, and `ports_test.sh` cannot signal a pid it did not spawn (ADR 0025). Read a `scripts/lib` test's stubs before running it, rather than assuming it is hermetic.
- **Never `pgrep` to wait.** Coding-agent sessions share the host process namespace, so `pgrep -f "cargo test ..."` matches *every* session's tests, and such a poll wedges until the Bash timeout fires. For long commands use `run_in_background: true` and wait with `TaskOutput`, which is per-session and reliable. (`TaskOutput` and `TaskStop` replaced the older `BashOutput`/`KillBash`.) To wait in shell, capture the PID directly: `cargo test ... & BG=$!; wait $BG`.
- **Never run two `cargo test` invocations against the same worktree in parallel.** `lucidos-engine` is heavy, so two overlapping runs fork two rustc trees on one crate and OOM-kill the host. Run one at a time: either fully foreground, or fully `run_in_background: true` and then `TaskOutput`. Never both. Narrow the filter to a *specific test module* (`engine::agent_session::lifecycle::tests::`) rather than a broad prefix, which cuts runtime and blast radius even for a single run.
- **Bash exit 137 / 143 is a hard failure, not "task done".** 137 is SIGKILL (the OOM killer) and 143 is SIGTERM. When ANY tool returns either, the work is incomplete: narrow the scope, free memory, kill the parallel invocation, then retry. Never read the non-zero exit as confirmation that the validating step ran. The engine-side net is `classify_result` (`agent_session/lifecycle.rs`), which surfaces an empty Result as `ResponseFailed` so the UI shows a red dot rather than a silent "completed". Do not lean on it: fix the OOM by not running tests in parallel.
- **`Edit` needs a fresh `Read`.** Always Read immediately before Edit — an earlier Read in the same session doesn't count once context drifts, a sub-task intervenes, or a watcher/linter touches the file. On `File has been modified since read`, re-Read and re-Edit; do NOT retry the same Edit verbatim — the on-disk content has changed and the old `old_string` may no longer match.
- **Prefer `Glob`/`Grep` over `ls`/`grep` with absolute paths.** A remembered path goes stale the moment a refactor moves it. The shell form then fails with "no such file or directory" where `Grep pattern: 'pat'` finds the new location. It is cheaper too, skipping the shell hop and the Bash-output tokens a miss costs. Reach for them for any path you did not just see this turn.
- **Anchor cargo at the worktree root — never `cd ..` then cargo.** Your default cwd is the worktree root; `cd ..` from there lands at `.lucidos/worktrees/`, which has no `Cargo.toml`, and `cargo check -p lucidos-engine` then errors "could not find Cargo.toml". Run cargo from the worktree root, or pass `--manifest-path Cargo.toml` (or `crates/<name>/Cargo.toml`) if you must invoke it from elsewhere.
- **Quote globs in Bash — zsh fails the whole command on no match.** The Bash tool runs under zsh, where unquoted `ls rust-toolchain*` exits non-zero with `zsh: no matches found: rust-toolchain*` if zero files match — bash would pass the literal through, zsh aborts. Quote it (`ls 'rust-toolchain*'`) or use `Glob pattern: 'rust-toolchain*'` for a real match. The error mentions `zsh` and looks like a tool problem, not a shell-syntax one.
- **"Still happening" means the fix is wrong, not unapplied.** When the user reports a behavior is STILL happening after your fix, assume the pending change is already applied. Never deflect with "it's not applied yet", "you're on a stale build", or "reload the PWA". The user does not report a bug as still-existing unless they have already clicked Apply and refreshed. Treat it as evidence the fix is wrong or incomplete, and re-read the code instead of speculating about the build.
- **Verify applied-state deterministically, not by guessing.** Run from the worktree: `git merge-base --is-ancestor <commit-or-HEAD> main && echo APPLIED || echo PENDING`. Frontend nuance: merged into main is applied, but the dev server still rebuilds `dist/` and an iOS PWA needs a refresh. This is not a speculation crutch, and it never overrides the bullet above.

## Code Style — Core Principles

- **Fix root causes, not symptoms.** Trace a bug to its originating layer; never paper over it with a downstream filter.
- **Make impossible states impossible.** Model state with enums, newtypes, controlled transitions. Where the platform cannot (DOM vs JS), reconcile and test.
- **KISS.** Resist special cases, flags, branches. Leave a file cleaner than you found it.
- **Test-driven.** Write tests first. A bug fix reproduces with a failing test. Run the relevant suites before claiming done.
- **Filtered Rust tests need `--lib`**: `cargo test -p lucidos-engine --lib "filter"`. Without `--lib`, runs zero tests silently. Verify `running N tests` line is > 0.
- **Multiple test name filters need `--`**: `cargo test -p lucidos-engine --lib -- name1 name2 …`. Without the `--` separator, clap treats every name after the first as an unknown positional and fails with `error: unexpected argument 'X' found`.
- **Integration tests for refactors.** A refactor that changes data flow MUST have integration tests: unit tests miss wiring bugs.
- **Contract tests are sacred.** Never hand-edit `src/generated/`; regenerate with the `generate_*_file` writer tests, listed in `.claude/rules/testing.md` § Contract Tests. Each staleness test names its own command.
- **Zero warnings, errors, failing tests.** "Pre-existing" is never an excuse: if you see it, you own it.
- **Test selection by what changed:**
  - Rust (`.rs`, `Cargo.toml`, `Cargo.lock`, `.sql`) → **`make lint`** + engine tests.
    - `make lint` runs ShellCheck, `cargo fmt --all --check`, then clippy with the canonical `CLIPPY_FLAGS`. It is the repo's ONE lint gate and supersedes a bare `cargo check`: same compile, plus warnings-as-errors, plus a rustfmt-clean tree.
    - Nothing else runs it per change, since Apply merges straight to `main` and `/harden` is the only stage between. Shell-only or `Makefile`-only diffs → `make lint` alone.
    - **Run engine tests via `make test` (= `./scripts/test-engine.sh`), never bare `cargo test -p lucidos-engine`.** They need Postgres, and with no DB up every DB-backed test panics on connect and reports hundreds of false failures. The script provisions a disposable `lucidos-pg-test` container and exports `TEST_DATABASE_URL`.
    - Narrow filter: `./scripts/test-engine.sh -- -- <name>`. Touched the HTTP API surface? Also run `./scripts/e2e-api.sh`.
    - **A narrow filter cannot certify a merge resolution.** Take one side's implementation and the other side's tests, and the contradiction lands in a file the filter excludes. The implementation's own test module passes, while the test asserting the opposite contract never runs. Any merge that mixes sides runs the FULL suite.
  - TypeScript (`.ts`, `.tsx`) → `npx tsc --noEmit` + `cd crates/lucidos-app && npm test`
  - CSS (`crates/lucidos-app/src/**/*.css`) → `cd crates/lucidos-app && npx vite build`. **Not "no tests".** Nothing else in the gate parses CSS: `tsc` ignores it and Vitest never built it, so a syntax error lands on `main` clean. There it kills the shared build-watch's `vite build`, which keeps serving the previous `dist/` and republishes nothing. Every LATER frontend Apply then strands with "applied but not served yet", pointing at the build-watch rather than at the CSS. `vite build` is sub-second and is the exact command the build-watch runs, so it fails on what the watch fails on.
  - The engine's `crates/lucidos-engine/src/api/sdk_iframe.css` → `cd crates/lucidos-app && npm test`. It is `include_str!`d and served to app iframes, so it is OUTSIDE the Vite graph and `vite build` never reads it; a syntax error there ships silently rather than breaking a build. `styles/__tests__/engine-served-css-parses.test.ts` postcss-parses it under the ordinary Vitest run. Add a new engine-served stylesheet to that test's list in the same change that adds the `include_str!`.
  - Docs-only → no tests
  - Mixed Rust and TS → run both
- **Test locations** — Rust unit: inline `#[cfg(test)]`. TS unit: `*.test.ts` next to source. Browser e2e: `crates/lucidos-app/e2e/*.spec.ts` (`./scripts/e2e-browser.sh`). API e2e: `crates/lucidos-e2e/tests/` (`./scripts/e2e-api.sh`). Contract tests: `crates/lucidos-app/src/generated/`. See `testing.md`.
- **We build locally: GitHub Actions is RELEASE-ONLY.** Never add a workflow that compiles, lints, type-checks, or tests the tree per push or PR: `.github/workflows/` holds **release and delivery verification only**, and nothing in it *deploys*. The reason is structural: Lucidos is **not PR-based**, so a `pull_request` trigger never fires and a `push` one reports only *after* the change landed. The per-change gate is `/harden`, enforced by `.claude/hooks/pre-push.sh` and run at Apply when the marker is missing. A new gate therefore goes in **`/harden` Phase 4.5's test-selection table**, never in a workflow. Publishing to a lucidos.dev origin runs on the maintainer's machine, never from public CI: the Cloudflare credential a CI deploy needs also carries `dns_records:edit` and `zone:edit` on the zone (ADR 0031).
- **Ruthless refactoring + DRY + no dead code.** Fix unclear names, dead branches, copy-paste when you touch a file. Delete unused, never comment out or `_`-prefix.
- **No provider-specific instructions in code.** Use `web_search` for those.
- **No private data in shipping files.** Everything except `docs/plans/**` and `WORKSPACES.md` ships verbatim to the public mirror — test fixtures and comments included. Never use real personal/family/company-internal data or machine paths as examples; use the generic placeholders. Single source of truth: `.claude/rules/no-private-data.md` (enforced by `/harden`, `/harden-project`, and the release guard).
- **Public API parameter values are kebab-case.** Use values like `coding-agent`, not `coding_agent`; reserve snake_case for JSON/DB/Rust/TS field identifiers where that is the established contract.
- **Conventional commits**: `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`. No em dashes in the subject or body, same as everywhere else (`.claude/rules/no-em-dashes.md`); a `git commit -m` carrying one is blocked at the Bash hook.

## Engine Statelessness

Engine must be restartable at any time without losing user-visible state. PostgreSQL + filesystem are sources of truth; in-memory is cache or active runtime.

- No critical state only in memory. Active processes are expected to die on restart.
- In-flight threads resume after restart, but the resume is **gated on cause**, for crash-safety. A **user-initiated switch** auto-resumes **coding-agent** threads (via `--resume`) and **chat / trigger** threads (via `continue_chat`, the manual Continue button's entry point). A **crash / OOM / panic / agent-killed** engine does NOT auto-resume: it keeps the manual **Continue** affordance, so work that may have crashed the engine cannot loop. A thread parked awaiting an *AskUserQuestion* answer is preserved either way. The "already merged" check applies only to non-running sessions. See `engine/agent_recovery/recovery.rs` and `engine/chat/recovery.rs`.
- **The switch fingerprint is `cause = EngineShutdown` AND a device actor, both halves.** `switch_was_user_initiated` is the single definition, shared by the coding-agent and chat resume gates via the `SWITCH_TEARDOWN_ABORT_SQL` / `THREAD_START_EVENTS_SQL` fragments. A device actor alone is NOT enough. `AbortCause::StaleSettle` deliberately carries the actor of the user button that exposed a stuck row: an actor-only gate therefore reads a user *Stop* as a switch, and re-runs abandoned work. When adding an abort site, decide which half it carries.
- Orphaned resources recover on startup. Frontend reconnects gracefully — page reload preserves visible UI state.
- **Allowed ephemeral**: channel senders/receivers, process handles, cancellation signals, `pending_captures`.
- **Not allowed**: preferences only in `RwLock` without DB write, pending changes only in memory.

## Core Architectural Principles

Lucidos is prompt-first: events as single source of truth, versioned artifacts in Git, resumable workflows.

- Events are immutable, append-only, **named in past tense** (`MessageReceived`).
- Git is the artifact store but **never the authority** — events are.
- Events are checkpoints; on crash, reconstruct state from the last confirmed step.
- External edits are staged, not immediately authoritative.
- **Broadcast and subscribe — never direct coupling.** Backend: state changes go through EventBus; consumers (SSE, projections, side effects) subscribe. Frontend: state flows through signals + SSE; components react to signal changes, never reach into other components or store internals. If you import a consumer into a producer, you've broken the pattern.
- **System events are individual variants, never wrappers.** Each event type is its own `SystemEvent` enum variant. Grouping is provided by `aggregate()` ("trigger", "app", …), not by `event_type: String` discriminators.
- **Rust is the source of truth for event types.** `SystemEvent` + `#[serde(tag = "type", content = "data")]` produces the wire format. Frontend handles the same names — no translation layers, no meta-events, no "entity changed" wrappers.

## Environment Variables

The full reference is the **`lucidos-env-vars` skill**: every `LUCIDOS_*`,
provider and network-bind variable, with its resolution order and default. It
loads on demand rather than sitting in every session. Reach for it to launch or
configure the engine or gateway, or to debug startup, binding or credential
resolution. Reach for it to change a default, or to edit `net_config.rs`, the
`llm/` provider modules, `desktop.rs`, `install.sh`, or `scripts/lib/`. Add new
variables there.

One rule stays resident because it is safety-critical and must hold even when the
skill is not loaded:

- `DATABASE_URL` — Postgres connection string for the engine. **Never hardcode the URL or password into a `psql`/Python invocation** — the engine sets `PGUSER`/`PGPASSWORD`/`PGHOST`/`PGPORT`/`PGDATABASE` in every spawned subprocess (CC sessions, bash + python tools, scheduled scripts), so just run `psql -c '…'` bare. Putting the URL in argv leaks the password into the persisted `Bash` tool-call payload that the steps UI renders.

## Workspace Layout & Taxonomy

See `docs/taxonomy.md` for the full content taxonomy. Key points:

- `.lucidos/` — gitignored, ephemeral runtime/cache; can be rebuilt.
- `data/artifacts/` — git-tracked, NEVER auto-delete.
- `data/postgres/` — legacy per-workspace Postgres data, kept only on old workspaces until verified shared-Postgres migration + decommission.
- **Intent** = what the user wants (stable). **Knowhow** = how to achieve it (evolves). **Script** = code invoked by either.
- **Ownership**: everything lives with its consumer (inside `apps/`, `triggers/`, `knowhow/`).
- **Survivability test**: "Does this survive if I delete the app?" → top-level. "Only makes sense for this app?" → inside the app.
- Never use generic names like `app.md` or `knowhow.md` — name by what they describe.
- **Manifest** = user-facing UI. **Knowhow / intents** = engine-facing LLM context.
- **Triggers: keep procedure out of intent.** A trigger's `run.intent` is what the user would say ("notify me when X happens"). Every imperative verb about *how* (hit, parse, scan, fall back, retry, emit) belongs in a knowhow file. The trigger thread looks up knowhow itself via `load_knowhow` at fire time, exactly as chat does, so there is no per-trigger allow-list. The trap is that the HTTP API takes one big string and will not stop you dumping the recipe in. See `docs/taxonomy.md` § Triggers.

## One-Click Install

Self-contained install, with no terminal, Docker, or dev tools required. Two shipped shapes: the **macOS `.app`/`.dmg`** (Apple Silicon only) and the **headless tarball** the `curl … | sh` installer lays down (macOS and Linux, both architectures). There is **no Windows build and no Linux AppImage**: `install.sh` refuses any OS other than Darwin or Linux. Bundled: PostgreSQL with pgvector, the engine binary, the static frontend. No OS-specific commands in the engine, which reaches Postgres over TCP. `.lucidos/` is rebuildable, and `artifacts/` plus Postgres is the complete state.

## Maintaining This File

When a new convention or architectural decision is established, update this file or the appropriate `.claude/rules/` file in the same session. Recipe-maintenance rules for `system-knowhow/` live in `.claude/rules/system-knowhow.md`.
