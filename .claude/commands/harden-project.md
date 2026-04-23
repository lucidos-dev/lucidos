Harden the entire project: scan ALL source files for bugs, dead code, CLAUDE.md violations, DRY violations, and code quality issues. Fix everything found. Full codebase sweep — not scoped to a branch diff.

## Phase 1: Build Checklists

Read the project rules and build two focused checklists — one for Rust agents, one for frontend agents:

- `CLAUDE.md` — core principles, code style
- `.claude/rules/rust.md` — Rust conventions
- `.claude/rules/frontend.md` — frontend conventions

For each checklist, list the specific patterns agents must scan for. Examples:

**Rust patterns**: `.ok()` on important errors, byte-index string slicing (`&s[..n]`), raw `println!`/`eprintln!` instead of `log!`, thread events not through EventBus, missing `?` propagation, `let _ =` on recoverable errors, missing serialization derives, path traversal vulnerabilities.

**Frontend patterns**: missing `Loadable<T>` state handling, `getElementById()`, `px` instead of `rem`, native `<select>` instead of `Dropdown`, system dialogs instead of `showToast`/`showConfirm`, fire-and-forget promises without `.catch()`, `loaded ? data : []` masking load state, direct component coupling instead of signals/SSE.

## Phase 2: Launch Module Agents

Split the codebase into 8 non-overlapping chunks. Launch ALL 8 agents **in a single message** so they run in parallel.

### Backend (Rust) — include the Rust checklist

| Chunk | Directories |
|-------|-------------|
| core | `crates/cognos-engine/src/core/` |
| engine | `crates/cognos-engine/src/engine/` |
| api | `crates/cognos-engine/src/api/` + top-level `crates/cognos-engine/src/*.rs` |
| llm-infra | `crates/cognos-engine/src/llm/`, `crates/cognos-engine/src/mcp/`, `crates/cognos-engine/src/memory/`, `crates/cognos-engine/src/runtime/`, `crates/cognos-engine/src/scheduler/`, `crates/cognos-engine/src/triggers/` |

### Frontend (TypeScript/CSS) — include the frontend checklist

| Chunk | Directories |
|-------|-------------|
| components-core | `crates/cognos-app/src/components/chat/`, `crates/cognos-app/src/components/layout/`, `crates/cognos-app/src/components/shared/`, `crates/cognos-app/src/components/drawer/` |
| components-features | All other subdirs under `crates/cognos-app/src/components/` NOT in components-core |
| store | `crates/cognos-app/src/store/` |
| frontend-infra | `crates/cognos-app/src/hooks/`, `crates/cognos-app/src/utils/`, `crates/cognos-app/src/api/`, `crates/cognos-app/src/styles/`, + top-level `crates/cognos-app/src/*.{ts,tsx,css}` |

**Exclude from all agents**: `crates/cognos-app/src/generated/` — auto-generated, never hand-edit.

### Agent Prompt

Each agent prompt must be **self-contained** (agents have no conversation context). Include:

1. **Role**: "You are hardening the `{chunk}` module of CognOS."
2. **Directories**: exact paths this agent owns
3. **Rules checklist**: the full language-specific checklist from Phase 1
4. **5-point scan** — check every file for ALL of these:
   - **Rules compliance** — violations of the checklist rules
   - **Dead code** — unused functions, imports, types, exports, variables → delete entirely (don't comment out or `_` prefix)
   - **Bug patterns** — code that will crash or produce wrong results
   - **DRY violations** — duplicated logic within the module → extract into shared function
   - **Code quality** — unclear names, unnecessary complexity, stale/wrong comments → fix
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

## Phase 4: Verify

Run tests based on which file types changed:

- `.rs` files changed → `cargo check -p cognos-engine && cargo test -p cognos-engine`
- `.ts`/`.tsx` files changed → `cd crates/cognos-app && npx tsc --noEmit && npm test`
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
