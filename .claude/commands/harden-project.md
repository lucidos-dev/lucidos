Harden the entire project: scan ALL source files for bugs, dead code, CLAUDE.md violations, DRY violations, and code quality issues. Fix everything found. Full codebase sweep — not scoped to a branch diff.

## Phase 1: Build Checklists

Read the project rules and build two focused checklists — one for Rust agents, one for frontend agents:

- `CLAUDE.md` — core principles, code style
- `.claude/rules/rust.md` — Rust conventions
- `.claude/rules/frontend.md` — frontend conventions
- `.claude/rules/no-private-data.md` — applies to BOTH checklists (it ships publicly; the scan must flag private/personal/company-internal data in any shipping file)

For each checklist, list the specific patterns agents must scan for. Examples:

**Rust patterns**: `.ok()` on important errors, byte-index string slicing (`&s[..n]`), raw `println!`/`eprintln!` instead of `log!`, thread events not through EventBus, missing `?` propagation, `let _ =` on recoverable errors, missing serialization derives, path traversal vulnerabilities.

**Frontend patterns**: missing `Loadable<T>` state handling, `getElementById()`, `px` instead of `rem`, native `<select>` instead of `Dropdown`, system dialogs instead of `showToast`/`showConfirm`, fire-and-forget promises without `.catch()`, `loaded ? data : []` masking load state, direct component coupling instead of signals/SSE.

## Phase 2: Launch Module Agents

Split the codebase into 8 non-overlapping chunks. Launch ALL 8 agents **in a single message** so they run in parallel.

### Backend (Rust) — include the Rust checklist

| Chunk | Directories |
|-------|-------------|
| core | `crates/lucidos-engine/src/core/` |
| engine | `crates/lucidos-engine/src/engine/` |
| api | `crates/lucidos-engine/src/api/` + top-level `crates/lucidos-engine/src/*.rs` |
| llm-infra | `crates/lucidos-engine/src/llm/`, `crates/lucidos-engine/src/mcp/`, `crates/lucidos-engine/src/memory/`, `crates/lucidos-engine/src/runtime/`, `crates/lucidos-engine/src/scheduler/`, `crates/lucidos-engine/src/triggers/` |

### Frontend (TypeScript/CSS) — include the frontend checklist

| Chunk | Directories |
|-------|-------------|
| components-core | `crates/lucidos-app/src/components/chat/`, `crates/lucidos-app/src/components/layout/`, `crates/lucidos-app/src/components/shared/`, `crates/lucidos-app/src/components/drawer/` |
| components-features | All other subdirs under `crates/lucidos-app/src/components/` NOT in components-core |
| store | `crates/lucidos-app/src/store/` |
| frontend-infra | `crates/lucidos-app/src/hooks/`, `crates/lucidos-app/src/utils/`, `crates/lucidos-app/src/api/`, `crates/lucidos-app/src/styles/`, + top-level `crates/lucidos-app/src/*.{ts,tsx,css}` |

**Exclude from all agents**: `crates/lucidos-app/src/generated/` — auto-generated, never hand-edit.

### Agent Prompt

Each agent prompt must be **self-contained** (agents have no conversation context). Include:

1. **Role**: "You are hardening the `{chunk}` module of Lucidos."
2. **Directories**: exact paths this agent owns
3. **Rules checklist**: the full language-specific checklist from Phase 1
4. **6-point scan** — check every file for ALL of these:
   - **Rules compliance** — violations of the checklist rules
   - **Dead code** — unused functions, imports, types, exports, variables → delete entirely (don't comment out or `_` prefix)
   - **Bug patterns** — code that will crash or produce wrong results
   - **DRY violations** — duplicated logic within the module → extract into shared function
   - **Code quality** — unclear names, unnecessary complexity, stale/wrong comments → fix
   - **Private data** — per `.claude/rules/no-private-data.md`, any real personal/family/company-internal data or machine path in a shipping file (test fixtures and comments included) → replace with the approved generic placeholder; leave legitimate attribution (the carve-out) alone
5. **How to work**:
   - Use Glob to list all source files in your directories
   - Read every file
   - Fix issues directly with Edit
   - Do NOT commit — the orchestrator handles commits
   - Do NOT touch files outside your directories
   - Cross-module issues: note in summary but don't fix
   - Only fix clear, unambiguous violations — not subjective preferences
   - Preserve existing behavior — no feature additions
6. **Output format**: End with a structured summary:
   ```
   ## Fixes Applied
   - file.rs:42 — [dead-code] removed unused function `foo`
   - file.rs:87 — [bug] added `?` to propagate error from `bar()`

   ## Cross-Module Issues (not fixed)
   - `pub fn baz` in core/events.rs appears unused outside core

   ## Clean
   (if no issues found)
   ```

## Phase 3: Cross-Module Sweep

After all 8 module agents complete, launch ONE agent to find cross-module issues:

- **Dead exports**: `pub` (Rust) or `export` (TypeScript) items only referenced at their definition, never imported elsewhere
- **Broken imports**: references to items that module agents may have deleted
- **Duplicate definitions**: same type or function defined in multiple modules

Use Grep across `crates/` to verify each item. Be conservative — only fix items confirmed dead.

## Phase 3.5: Temporary-measures Reconciliation

The whole-tree face of the **temporary-measures & marker-hygiene** check (the
per-change face lives in `/harden` Phase 2 Agent 2 — same registry, same markers,
same inclusion test, same "register it" escape valve; this one just sweeps the
whole tree instead of a diff). It is a **reporting** pass — surface findings in the
final summary; don't silently delete impermanent code or registry rows.

Read `docs/temporary-measures.md` (governed by `.claude/rules/temporary-measures.md`),
then check **both directions**:

1. **Overdue cleanup — entries whose removal condition appears already met.** For
   each `active` / `open` entry, read its removal/resolution condition and check the
   tree for whether it's now satisfiable: the upstream bug it worked around is
   fixed, the feature flag's cleanup is done, the model reliably emits the canonical
   form, the investigation looks closeable. Flag any entry that reads as ready to
   retire — with the evidence — so a human can flip its status to `removed` /
   `resolved` (kept as history, never deleted) and do the paired cleanup the entry
   names. Closing an investigation flags every measure tagged with its id.

2. **Impermanent code missing a row.** Grep the tree for impermanent-looking things
   NOT in the registry: `TODO` / `FIXME` / `HACK` / `XXX` markers, and
   `remove after` / `temporary` / `diagnostic-only` / `workaround until` comments,
   feature flags / kill-switches, and sunset back-compat shims. For each that meets
   the inclusion test (*meant to go away, with a concrete condition for when*),
   flag it as needing a registry row — the fix is to **register it**, not to launder
   the marker. Skip anything on the rule's OUT list (permanent back-compat / old-data
   tolerance, site-local `#[allow(...)]` / `@ts-expect-error` / `eslint-disable`
   suppressions, ADR-recorded design decisions, open-ended tech debt with no concrete
   end condition) — those are tracked elsewhere or not at all.

## Phase 3.6: Packaged-Runtime Dependency & Fail-Fast Sweep

A **reporting** cross-cutting pass (like Phase 3.5): surface findings in the final summary; don't silently rewire spawns or staging. It generalizes the bug class catalogued in `data/artifacts/audits/packaged-app-bundle-and-failfast-audit.md` — a runtime dependency that resolves in dev (rich shell PATH + `target/{debug,release}/` binaries side-by-side) but is absent or unreachable in the packaged macOS `.app` / headless tarball / `install.sh` service (minimal launchd PATH `/usr/bin:/bin:/usr/sbin:/sbin` + a fixed staged `RESOURCE_NAMES` set), failing as a cryptic mid-stream tool error or an indefinite boot-splash hang instead of a clear message.

First establish the ground truth — read what is actually staged and which env vars the packaged runtime sets:

- `scripts/lib/stage_runtime.sh` — `RESOURCE_NAMES` + `stage_runtime_assemble` (the staged set)
- `crates/lucidos-app/src/desktop.rs` — `spawn_gateway` env + `bundled_resources`
- `scripts/lib/service.sh` — `service_runtime_env_pairs` (the install-service env)

Then sweep the engine + gateway + CLI (`crates/lucidos-engine/src/`, `crates/lucidos-gateway/src/`, `crates/lucidos-cli/src/`) for both directions:

1. **Pattern A — unstaged or PATH-dependent runtime deps.** Grep for `Command::new(`, MCP server `command:` entries, hook `command`s, `include_str!`/`include_bytes!` vs disk reads, `LUCIDOS_*_DIR` / `*_BIN` env reads, and `current_exe()`-relative walks. For each, classify: resolved by absolute path / staged-dir env var (safe), or bare name relying on PATH. Flag any bare-name spawn whose target is not on the launchd minimal PATH and not staged, and any disk-read asset that no delivery vehicle stages. A genuinely external user-install is acceptable only if probed like `resolve_claude_binary` AND fail-fast on absence.
2. **Pattern B — silent degrade instead of fail-fast.** Flag any binary/dir resolution that returns `None`/`Err` and proceeds anyway, any "is the env var set?" check that never stats the path, any health/readiness check that reports healthy while a required surface is missing, and any spawn whose missing dependency can only surface mid-stream.
3. **Delivery-vehicle drift.** Confirm `RESOURCE_NAMES` + the env contract are identical across `build-dmg.sh`, `build-headless.sh`, `stage_runtime.sh`, `desktop.rs::spawn_gateway`, and `service.sh::service_runtime_env_pairs` — and that the `--check` validators assert the runtime-required set, not just the resource map's self-consistency.

## Phase 4: Verify

Run tests based on which file types changed:

- `.rs` files changed → `cargo check -p lucidos-engine && cargo test -p lucidos-engine`
- `.ts`/`.tsx` files changed → `cd crates/lucidos-app && npx tsc --noEmit && npm test`
- Both → run both

If tests fail: diagnose root cause, revert or adjust aggressive fixes. All tests must pass.

## Phase 5: Commit

If any files were changed:

```bash
git add <changed-files>
git commit -m "chore: project-wide harden"
```

Report the full summary across all modules — every fix with file, line, category, and description.

If no issues found: "Project is clean — no issues found."
