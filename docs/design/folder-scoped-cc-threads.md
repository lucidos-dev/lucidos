# Folder-scoped Claude Code threads

**Status:** Design — not implemented. The user reads this and decides what to build.
**Date:** 2026-05-25
**Glossary terms used:** coding-agent thread, agent session, Claude Code session, worktree, repository, app, workspace, change, hardening, Apply.

## Problem

The `run_claude` LLM tool today takes a `repo` parameter resolved through `manage_repositories`. Every session is "a registered git repo + an isolated worktree of it". That model fits editing the Lucidos source tree cleanly. It does NOT fit editing a Lucidos **app** that lives in a workspace data dir (`<workspace>/data/apps/<name>/`), because:

- The user's "I want CC to fix habit-tracker" intent is "edit a folder", not "edit a registered repo".
- Each workspace's `data/` is its own git, so commits already land in the right destination if CC just edits the folder in place.
- The current workaround — omit `repo`, use absolute paths in the prompt, let CC edit outside the worktree — makes the worktree abstraction a lie: `git diff main` in the worktree is empty, the Apply button has nothing to apply, and `/harden` Phase 4.5 scans a worktree with zero relevant changes.

The user's framing: *"we don't really have a setup for cc … they should not take repo but folder. Maybe we should always just do folder."*

This doc maps where "session = repo + worktree" is baked in today, proposes a folder-scoped model, and grills it.

---

## 1. Current state map

Every site that assumes the repo+worktree model, with file:line refs.

### 1.1 LLM tool surface

| Surface | Location | What it bakes in |
|---|---|---|
| `run_claude` schema | `crates/lucidos-engine/src/llm/tools/mod.rs:1346-1391` | Optional `repo` string param. Description: *"REQUIRED whenever the work targets anything outside the Lucidos source tree"*. Omit ⇒ Lucidos. |
| `run_claude` handler | `crates/lucidos-engine/src/engine/agentic_loop_special_tool.rs:245-357` | Reads `repo` arg, calls `RepositoryStore::resolve(name_or_id)` (line 285), passes resolved `repo_id` into `spawn_agent_thread` (line 340). |
| `manage_repositories` schema | `crates/lucidos-engine/src/llm/tools/mod.rs:183-211` | Actions `add` / `list` / `remove`. The companion to `repo=`. |
| `manage_repositories` handler | `crates/lucidos-engine/src/engine/tools/mod.rs:487-592` | Validates path exists (line 531), runs `git rev-parse --git-dir` (line 535-546). Refuses non-git folders. |
| `RepositoryStore` | `crates/lucidos-engine/src/core/repositories.rs:15-95` | `list`, `get(uuid)`, `get_by_name`, `resolve(id_or_name)`, `add`, `remove`, `ensure_exists`. The single source of truth for "what can CC be spawned against?". |
| HTTP endpoints | `crates/lucidos-engine/src/api/repositories.rs:28-140` | `GET/POST/DELETE /api/v1/repositories`. Validates `~/` expansion, path existence, `git rev-parse --git-dir`. |

### 1.2 Repository registry — storage

| Surface | Location | Notes |
|---|---|---|
| Schema | `crates/lucidos-engine/migrations/20260318000000_create_repositories.sql:1-7` | `id UUID, name TEXT, path TEXT UNIQUE, description TEXT, created_at TIMESTAMPTZ`. No "kind" or "git-root vs folder" column. Implicitly, every row IS a git root. |
| Default fallback | `crates/lucidos-engine/src/engine/engine_impl.rs:7` | `DEFAULT_REPO_NAME = "Lucidos"`. When `repo` is omitted, `RepositoryStore::get_by_name("Lucidos")` resolves it. |

### 1.3 Worktree creation

| Surface | Location | Notes |
|---|---|---|
| Repo → worktree spawn | `crates/lucidos-engine/src/engine/agent_session/run_session.rs:180-224` | `repo_id.is_some()` ⇒ fetch from `RepositoryStore`; else fall back to `DEFAULT_REPO_NAME`. Builds `(repo_id, repo_root, is_external_repo, external_repo_name, repo_name)` tuple — every session has exactly one repo. |
| `is_external_repo_path` | `crates/lucidos-engine/src/engine/git_ops.rs:130-135` | Canonicalize-compare against `dev_root` (the Lucidos source). Drives system-prompt selection and the "no Apply event" external-repo flow. |
| Worktree dir | `crates/lucidos-engine/src/engine/git_ops.rs:8-18` | `<workspace>/.lucidos/worktrees/`. Per-thread subdir `thread-<8-char-id>` via `resume.rs:51-58`. |
| `git worktree add` | `crates/lucidos-engine/src/engine/git_ops.rs:626-648` | `--no-checkout`, symlinks git-crypt if present, `checkout HEAD`. |
| Branch naming | `crates/lucidos-engine/src/engine/agent_session/spawn.rs:37-40` | `claude-code/<YYYYMMDD-HHMMSS>-<6-char-uuid>`. |
| Marker file | `crates/lucidos-engine/src/engine/claude_code.rs:360` | `.lucidos-workspace` written into every worktree, identifying its owning workspace. Used for orphan recovery. |
| Worktree excludes | `crates/lucidos-engine/src/engine/claude_code.rs:376-380` | `.lucidos-workspace`, `.claude/skills/lucidos-cli/`, `RUNTIME_PATH_PREFIX` — kept out of git in external repos. |

### 1.4 System prompts

| Surface | Location | Notes |
|---|---|---|
| Lucidos worktree prompt | `crates/lucidos-engine/src/engine/agent_session/prompts.rs:129-176` (`worktree_system_prompt`) | Hardcodes Lucidos terminology: "complete copy of the Lucidos repository", "engine pushes to remote after the user clicks Apply", `scripts/web-dev.sh`, `cargo build`, the restart/Apply rule, the harden rule, no-pull-request rule. |
| External repo prompt | `crates/lucidos-engine/src/engine/agent_session/prompts.rs:179-199` (`external_repo_system_prompt`) | Minimal: "you have full git access. Create feature branches, push, and create PRs as needed." No Apply, no restart, no harden. |
| Recovery variants | `crates/lucidos-engine/src/engine/agent_session/prompts.rs` (`recovery_system_prompt`, `external_repo_recovery_system_prompt`) | Selected at `run_session.rs:246-252` by `external_repo_name.is_some()`. |

### 1.5 Cross-workspace dispatch

| Surface | Location | Notes |
|---|---|---|
| Outbound (`workspace=` set) | `crates/lucidos-engine/src/engine/agentic_loop_special_tool.rs:540-595` → `crates/lucidos-engine/src/engine/http/workspace_client.rs:117-171` | POSTs to `localhost:<port>/api/v1/chat/stream` on the target workspace. |
| Inbound | `crates/lucidos-engine/src/api/chat.rs:497-514` | Resolves `repo_id` (string) via `RepositoryStore::get_by_name` in the **TARGET** workspace's pool. Unknown name ⇒ 400. The target trusts the caller to have asked for a repo it knows. |
| Trust model | `crates/lucidos-engine/src/engine/http/workspace_client.rs:1-10` | Comment: *"receiving engine treats these caller fields as a display hint only — they are user-controllable and MUST NOT be used for authorization."* Filesystem trust only: you must read `~/workspaces/<name>/.lucidos/ports`. No shared secret. |

### 1.6 Apply path

| Surface | Location | Notes |
|---|---|---|
| HTTP endpoint | `crates/lucidos-engine/src/api/changes.rs:164` (`apply_change`) | Routes to `Engine::apply_change(id, actor)`. |
| Core logic | `crates/lucidos-engine/src/engine/change_ops.rs:406` | If `hardened` ⇒ `ff_merge_to_main`. Else spawn hardening, retry. Returns `Noop | Conflict | Hardening | Applied`. |
| `ChangeProposed` emission | `crates/lucidos-engine/src/engine/change_ops.rs:304-370` (`propose_change`) | Carries `branch_name`, `repo_root`, `description`, `files`, `requires_restart`, `hardened`, `incomplete`. |
| `repo_root` populated from | `change_ops.rs:357` | `ProposeChangeInput.repo_root` supplied by the agent session at proposal time. Stored on the row and used at apply time. |
| `ff_merge_to_main` | `crates/lucidos-engine/src/engine/git_ops.rs:463-500` | Mutex-serialised fast-forward merge with rebase-catchup retry. Removes the worktree, deletes the temp branch, pushes main in background. |
| `files_require_restart` | `crates/lucidos-engine/src/engine/git_ops.rs:140-158` | Hardcoded Lucidos layout: `.rs`/`Cargo.toml`/`Cargo.lock` (excluding tests/docs), `*/migrations/*.sql`, `packages/lucidos-sdk/`, `crates/lucidos-engine/src/api/sdk_iframe.*`. **Always returns false for any path outside the Lucidos source layout** — including app folders and external repos. |
| `changes` table columns | migrations through `20260507064229_add_incomplete_to_changes.sql` | `id, session_id, branch_name, repo_root, description, file_count, files, requires_restart, status, created_at, resolved_at, merge_worktree_path, merge_temp_branch, thread_id, hardened, commits, incomplete`. Every row assumes a feature branch off main inside a single repo root. |

### 1.7 Harden path

| Surface | Location | Notes |
|---|---|---|
| Skill file | `.claude/commands/harden.md` | Phases 0–5. Phase 4.5 has a **hardcoded** table: `.rs/Cargo.toml/Cargo.lock/.sql` → `cargo check && cargo test -p lucidos-engine`; `.ts/.tsx` → `cd crates/lucidos-app && npx tsc --noEmit && npm test`. CSS-only / docs-only → skip. No support for "this folder isn't Lucidos". |
| `ALREADY_HARDENED` marker | `.claude/hooks/mark-harden.sh` → `lucidos hardened mark` → `POST /api/v1/internal/mark-hardened` → `record_hardened()` at `crates/lucidos-engine/src/api/internal.rs:266` | Stored in `hardened_branches` table, keyed by canonical `(repo_root, branch_name, head_sha)`. The branch-name key assumes a per-session feature branch. |
| Query | `crates/lucidos-engine/src/api/internal.rs:306` (`query_hardened`) | Reports `FRESH` / `STALE` / `MISSING`. STALE = HEAD has advanced since the mark; the harden skill re-runs. |

### 1.8 Thread UI

| Surface | Location | Notes |
|---|---|---|
| Branch chip | `crates/lucidos-app/src/components/chat/MessageRoutePanel.tsx:85-138, 437-440` | Reads `branch` from `SessionStarted` / `ContinuationStarted`. |
| Pending change badge | `crates/lucidos-app/src/components/thread/ThreadStatusIcon.tsx:17,40-41` | Driven by `ThreadMeta.codingAgentProposed` (projection: `thread_summaries`), set by `ChangeProposed`, cleared by `ChangeApplied`/`ChangeDiscarded`. |
| Change panel | `crates/lucidos-app/src/components/.../ChangesView.tsx` | Calls `getChangeDiff(changeId)` and `getThreadCcDiff(threadId)`. |
| `thread_summaries` worktree fields | `crates/lucidos-engine/src/core/store/threads.rs:140-143, 259` | `cc_repo_id: Option<String>` (UUID of external repo, NULL for Lucidos) and `coding_agent_has_diff: bool`. No `folder` column. |
| `SessionStarted` event | `crates/lucidos-engine/src/engine/thread_events.rs:947-955` | `{ session_id: String, branch: String, repo_id: Option<String> }`. **`branch` is mandatory**, `folder` does not exist. |
| Repo dropdown | `crates/lucidos-app/src/components/chat/PromptInput.tsx:835-843` | `Dropdown` bound to `selectedRepoId` signal. Persisted to `localStorage` as `lucidos-cc-last-repo`. Sent on the chat request as `repo_id`. |
| Prose mentioning worktree/branch/repo | `DiskUsagePage.tsx:212`, `WaitingBanner.tsx:13,241`, `RepoFilePreview.tsx:34-36`, `worktree_system_prompt` (above) | Multiple UI strings; each must be revisited for "in-place" sessions where no branch / no worktree exists. |

### 1.9 Summary of bakedness

Every layer — tool schema, handler, session-spawn, worktree creation, system prompt selection, Apply event, harden marker, UI chip, `thread_summaries` projection — assumes a 1:1 of *session ↔ feature branch on a registered git repo*. The folder-scoped model has to either preserve this 1:1 by faking a branch for in-place sessions, or relax it so a session can have `folder` without a `branch`.

---

## 2. Proposed model

### 2.1 Tool parameter

`run_claude` takes `folder: string` instead of `repo`. `repo` accepted as a deprecated alias that resolves through `RepositoryStore::resolve(repo)` into `folder = repo.path`.

Accepted `folder` values:

1. **Absolute path** — `/Users/.../workspaces/myws/data/apps/habit-tracker` or `~/IdeaProjects/lucidos/crates/lucidos-engine/src/api`.
2. **Registered repo name or UUID** — back-compat. Resolved to `repo.path`.
3. **Workspace-relative path** — `data/apps/habit-tracker` (resolved against the *target* workspace's root, same workspace if `workspace=` is omitted).

The folder is the **scope** — what CC is being asked to edit and where its cwd lands. The engine decides what to *do* with that folder (worktree it, edit in place, refuse) based on **detected mode** (§2.3).

### 2.2 Registry shape

Keep the `repositories` table as-is. **It remains the list of git roots eligible for the worktree-and-merge flow.** App folders, data subdirs, and arbitrary paths do NOT need to be registered — they're addressable by literal path.

We do not need a parallel "folders" registry: workspace data dirs are already discoverable (`<workspace>/data/`) and apps follow the convention `<workspace>/data/apps/<name>`. If we want LLM-side discoverability, add a `list_folders` query inside `manage_repositories` later — out of scope for v1.

### 2.3 Session modes — detected from the folder

The engine maps `folder` to one of three modes. **Coding-agent threads on Lucidos apps are treated as "serious" work and get the same worktree + change + Apply gate that Lucidos source edits get**, because if the user is reaching for a coding agent the work is significant enough to warrant a staging gate. Workspace data is a peer of Lucidos source in this design — same machinery, different repo root.

| Mode | When | Worktree? | cwd | Commits land | Apply | /harden discipline |
|---|---|---|---|---|---|---|
| `worktree` | Folder is (a) inside the registered **Lucidos** source tree, (b) the root of a registered external repo, OR (c) inside the **target workspace's `data/` tree** | Yes — `<ws>/.lucidos/worktrees/thread-<id>/` of the enclosing git | Worktree-relative path matching the requested folder | On a fresh `claude-code/...` branch off the enclosing git's main | **Lucidos:** ff-merge to main, `files_require_restart` evaluated, may restart engine. **Data (`<ws>/data/`):** ff-merge to data git's main, no engine restart, optionally emits `AppUiRefreshRequested` for any touched apps. **External repo:** skipped (today's behavior — no `ChangeProposed`; user pushes manually). | **Lucidos:** full Lucidos skill, marker enforced at Apply. **Data:** Lucidos's `/harden` does **not** run — the app brings its own test/lint setup (per the user's directive: "we don't use our CLAUDE.md steps like harden for apps; they have their own setups of tests etc"). Engine does not enforce a `hardened_branches` marker for data changes; Apply clicks through without that check. **External repo:** none (external repo's own CI). |
| `in_place` | Folder is inside an **unregistered** git that is neither Lucidos source nor under workspace `data/` | No | The folder itself | On the unregistered git's current branch — each CC commit lands immediately | No | None |
| `no_git` | Folder is not inside any git, AND not a system path (rejected) | No | The folder itself | Nowhere (no git) — edits persist on disk only | N/A | None |

**Mode detection algorithm** (pseudocode):

```
folder_abs = canonicalize(folder, base = target_workspace_root_if_relative)

# Refusals first
if folder_abs is inside <target_ws>/.lucidos/  → REFUSE (gitignored cache)
if folder_abs is /, ~, ~/.ssh, /etc, /System, ... → REFUSE (system path)
if folder_abs does not exist                       → REFUSE

# Mode selection — all three worktree sub-flavors take the same `worktree` branch;
# the only thing that varies is which repo_root the worktree wraps and what Apply does.
registered_lucidos = RepositoryStore::get_by_name("Lucidos").path  (resolves to dev_root)
target_ws_data     = <target_ws>/data/

if folder_abs starts_with registered_lucidos      → mode = worktree, kind = lucidos,  repo_root = registered_lucidos
elif folder_abs is exactly some_registered_repo.path → mode = worktree, kind = external, repo_root = repo.path
elif folder_abs is inside (or equal to) target_ws_data → mode = worktree, kind = data,    repo_root = target_ws_data
else:
    toplevel = git rev-parse --show-toplevel inside folder_abs
    if toplevel exists → mode = in_place, repo_root = toplevel   (rare; see Q4)
    else                → mode = no_git
```

Three subtleties:

- **Subdir of a registered repo / data dir:** `worktree` mode wraps the enclosing git; CC's cwd is the subdir within the worktree. CC sees the whole tree but starts narrowed.
- **Data git is a real git.** Each workspace's `data/` is its own git repo (separate from the Lucidos source tree); `git worktree add` on it produces an isolated copy at `<workspace>/.lucidos/worktrees/thread-<id>/`. Branches like `claude-code/<ts>-<uuid>` live on the data git, ff-merge to its main, and accumulate the same `git log` history Lucidos source does (see Risk #4).
- **Unregistered git outside both Lucidos and `data/`:** falls into `in_place`. The minority case — user can register the repo to upgrade. See Q4.

### 2.4 Apply semantics per mode

- **worktree + Lucidos:** unchanged from today. `ChangeProposed` emitted (`repo_root = Lucidos source path`), Apply button shown, `ff_merge_to_main` on accept, `files_require_restart` evaluated (may surface "Apply & Restart"), engine restart if needed.
- **worktree + data:** symmetric with Lucidos but on the workspace's data git. `ChangeProposed` emitted with `repo_root = <workspace>/data/`, `branch_name = claude-code/...`, `files = [...]`. Apply ff-merges the CC branch into the data git's main. `files_require_restart` returns false for data-tree paths (the engine binary doesn't depend on data files; an `AppUiRefreshRequested` transient event may fire for any touched apps so open app iframes pick up the new manifest/script without a full reload).
- **worktree + external:** unchanged from today. No `ChangeProposed`. CC commits to its branch; user uses `gh`/`git push` if they want it elsewhere.
- **in_place:** **no** `ChangeProposed`, **no** Apply button. Each CC `git commit` lands on the unregistered git's current branch and is broadcast via the existing tool-event stream. The thread UI shows commits, not a pending-change row.
- **no_git:** the system prompt warns CC that nothing is tracked. CC may still write files. No commit, no Apply.

### 2.5 Harden semantics per mode

- **worktree + Lucidos:** unchanged from today — full Lucidos `/harden` skill (Phases 0–5), marker enforced; Apply gates on `FRESH` marker (else runs harden synchronously).
- **worktree + data:** Lucidos's `/harden` **does not run**. Apps own their test/lint stories; the engine does not impose its hardening pipeline on them and does not gate Apply on a `hardened_branches` marker. The harden hook (`mark-harden.sh`) is not invoked; `query_hardened` is not called from the data Apply path. Apps that DO want a review pass can provide their own `.claude/commands/harden.md` (or any other command) inside the app folder — CC will pick it up via cwd discovery and run it on demand. The Apply path treats data-tree changes as `hardened = N/A` (a dedicated flag, not "false" — see Q13).
- **worktree + external:** Lucidos's `/harden` does not run. External's own CI is responsible.
- **in_place:** none.
- **no_git:** none.

### 2.6 UI implications

- `SessionStarted` event gains `folder: Option<String>` and `mode: SessionMode` (`worktree` / `in_place` / `no_git`). `branch` becomes optional (empty for `in_place` and `no_git`; populated for both Lucidos and data worktrees).
- `thread_summaries` gains `cc_folder: Option<String>` and `cc_mode: TEXT`. The existing `cc_repo_id` continues to point at registered external repos (NULL for Lucidos source AND for data worktrees) — a new `cc_kind` column (`lucidos` / `data` / `external` / `in_place` / `no_git`) is the cleanest distinguisher; alternatively, derive `kind` from `(cc_repo_id, cc_folder, cc_mode)` in the projection.
- Branch chip: data-tree threads look like Lucidos threads — same `claude-code/<ts>-<uuid>` chip, same Apply button. Add a small kind indicator next to the chip ("data: apps/habit-tracker" or just an app icon) so the user can tell what's being applied. `in_place` and `no_git` threads show a folder chip with no branch, and no Apply button.
- Change panel: shown for all `worktree`-mode threads (Lucidos, data, external — though external's "Apply" is a no-op today). Hidden for `in_place` / `no_git`.
- Repo dropdown in compose view: rename to "scope" picker. Top: Lucidos (default). Next: registered repos. Submenu: "App in this workspace" → `data/apps/*` plus "Other data folder" → picker for `data/triggers/*`, `data/knowhow/*`. The LLM-only path (absolute folder via tool args) doesn't need a UI affordance. Path persisted to `localStorage` as `lucidos-cc-last-folder` (back-compat: migrate `lucidos-cc-last-repo` once).

### 2.7 Cross-workspace

Unchanged in shape: outbound POSTs to target's `/api/v1/chat/stream`. Inbound resolves `folder` on its own filesystem and runs through §2.3 detection on its own data. If `folder` is a registered repo name, target uses ITS `RepositoryStore::resolve`. If absolute path, target evaluates against ITS filesystem. Cross-workspace inherits the same filesystem trust model (no shared secret, no auth).

### 2.8 Permissions / sandboxing

- `worktree` mode is naturally sandboxed in all three flavors (Lucidos, data, external) — CC cannot escape the worktree without absolute paths. For data worktrees this is the big win over the original in_place proposal: a session asked to edit `data/apps/habit-tracker` can technically still write to `<worktree>/apps/anything-else`, but the Apply review surface shows every file changed across the worktree, so accidental cross-app edits are visible at Apply time rather than silently landing on main.
- `in_place` mode gives CC write access to the entire unregistered git. **Risk:** the same cross-folder write risk as the old in_place mode, now scoped to the rare unregistered-git case. System-prompt warning only. Mitigation deferred to v2.
- `no_git` mode trusts the user's path choice. Refuse system paths (`/`, `~`, `~/.ssh`, `/etc`, `/System`, `/usr`). Refuse anything under `<workspace>/.lucidos/`.

---

## 3. Migration plan

### 3.1 Back-compat for `repo=`

- Engine accepts both `repo` and `folder` on `run_claude`. If both present, error: "pass exactly one of `repo` (deprecated) or `folder`".
- `repo` ⇒ `folder = RepositoryStore::resolve(repo).path`. Result: identical behavior to today.
- Tool schema description marks `repo` as deprecated and points at `folder`. The system prompt for the calling LLM (Lucidos Agent) is updated in the same change.

### 3.2 Default fallback

Today: missing `repo` ⇒ `DEFAULT_REPO_NAME = "Lucidos"`. Tomorrow: missing `folder` AND missing `repo` ⇒ same fallback (`folder = Lucidos repo path`). This preserves the no-arg case for Lucidos-source edits.

### 3.3 DB

- `SessionStarted` payload — additive: `folder: Option<String>`, `mode: SessionMode`. Old rows without these fields decode with defaults (`folder = None`, `mode = worktree` inferred from presence of `branch`).
- `thread_summaries` — additive: `cc_folder TEXT NULL`, `cc_mode TEXT NULL`, and `cc_kind TEXT NULL` (`lucidos` / `data` / `external` / `in_place` / `no_git`). Old rows null; the projection backfills from event replay.
- `changes` — unchanged in schema. Only `worktree`-mode sessions write to it. **The row's `repo_root` column now distinguishes Lucidos source (worktree path under `~/IdeaProjects/lucidos`) from a data git (`<workspace>/data/`)** — Apply must branch on that to decide whether `files_require_restart` should run.
- `hardened_branches` — unchanged in schema. Only **Lucidos** worktree sessions write to it (data worktrees skip the marker entirely; external worktrees never wrote it). Apply's "check marker, run harden if missing" path runs only when `repo_root == lucidos_dev_root`.

No data migration. All migrations are pure schema additions.

### 3.4 Frontend

- `selectedRepoId` signal renamed to `selectedFolder` (or kept and joined by a new signal — see Q7). Chat request payload gains `folder: string`; `repo_id: string` accepted for back-compat.
- Branch chip → folder-or-branch chip (one component, both display strategies).

### 3.5 Deprecation timeline

- v1: `folder` ships, `repo` accepted with a console warning in the engine log on every use.
- v2 (≥4 weeks later): `repo` removed from the tool schema. The handler still accepts it for one more release with a louder warning.
- v3: `repo` rejected.

The `manage_repositories` LLM tool stays — registered git roots are still the way to opt in to worktree-and-merge for non-Lucidos repos.

---

## 4. Open questions

Numbered. For each: the question, recommended answer, the trade-off.

### Q1. Should we always worktree, or sometimes edit in place?

**Decided (user directive, 2026-05-25):** **Always worktree for "serious" folders** — Lucidos source, registered external repos, and **anywhere under the workspace's `data/` tree**. The argument: if the user is invoking a coding agent on an app, the work is significant enough to deserve a staging gate; quick edits go via chat without spawning Claude Code. Casual chat edits to apps continue to commit directly through the chat path (out of scope for this doc); only coding-agent threads get the worktree.
**Recommendation:** worktree + Apply for every folder under `<workspace>/data/`.
**Trade-off:** Every coding-agent session on an app adds a `claude-code/<ts>-<uuid>` branch to the data git's `git log --graph --all`. After dozens of sessions the data git's branch history is noisy. Accepted in exchange for the review gate. The narrow case for in_place mode (unregistered git outside both Lucidos and the workspace data) survives — see Q4 — but it's the minority path.

### Q2. Does `folder` accept registered repo names, or only paths?

**Recommendation:** Accept both. Path is the canonical form; registered name is sugar.
**Trade-off:** Two ways to say the same thing is a foot-gun (ambiguity if a registered repo is named `data` and the user types `folder="data"`). Mitigation: name resolution only kicks in if the string does not look like a path (no `/`, no `~`, no `.`). This is mostly fine for today's "Lucidos" naming convention, but we should call it out in the tool description.

### Q3. What does Apply do for `worktree`-mode external repos?

**Recommendation:** Same as today: nothing. External repos do not get `ChangeProposed`; the user is expected to push/PR themselves.
**Trade-off:** Lost opportunity to give external repos the Apply ergonomics. But Apply's main value (fast-forward to main, restart engine) doesn't apply outside Lucidos. Adding a generic "Apply = merge to main of this repo" path is a separate feature; leave it out of v1.

### Q4. What about a folder inside an unregistered git repo (neither Lucidos nor under workspace data)?

**Recommendation:** `in_place` mode. Commits land on whichever branch the unregistered git is on. UI shows a folder chip with the toplevel path hinted, no Apply button. The user can register the repo later to upgrade to `worktree` mode.
**Trade-off:** Less safety than worktree (concurrent coding-agent threads on the same unregistered repo will race at commit time). Acceptable for the minority case — most "serious" work either targets Lucidos source, a registered external repo, or the workspace data tree, all of which get worktree+Apply. Alternative — refuse and demand registration — pushes friction onto the user for what's likely a one-shot edit. Worth revisiting if telemetry shows in_place is hit often.

### Q5. What does `/harden` do for data-tree worktree sessions?

**Decided (user directive, 2026-05-25):** **Lucidos's `/harden` does not run.** Apps own their test/lint discipline; the engine does not impose its hardening pipeline on them. The harden hook is not invoked; Apply does not query `hardened_branches` for data-tree changes; the `hardened` flag on the `changes` row is treated as N/A (not "false"). Apps that want a review pass can ship their own `.claude/commands/harden.md` (or other commands) inside the app folder for ad-hoc invocation by CC.
**Trade-off:** Apply on an app change is unconditional once CC commits — no automated check that the app's tests pass. The user owns that. Accepted because the alternative (a one-size-fits-all harden) is wrong for app-shaped work: cargo/npm aren't installed in many apps, tests are app-specific, and the marker concept is meaningless when the app has its own CI story.
**Open follow-up:** the system prompt for data-tree worktrees should NOT advertise `/harden` as mandatory (today's `worktree_system_prompt` includes the `HARDENING_RULE`). A new system prompt variant `data_worktree_system_prompt` is needed — see §3.3 / appendix.

### Q6. Concurrent sessions on overlapping paths?

**Recommendation:** Don't introduce a mutex in v1. With "everything under data/ gets a worktree" the concurrency story collapses to "two worktrees on the same data git" — same model as two CC sessions on Lucidos. Each worktree branches off main, both Apply paths serialise through the existing `MERGE_MUTEX` in `git_ops.rs`, and the second to Apply gets a rebase-catchup retry. The `in_place` minority case can still race; CC surfaces the commit failure.
**Trade-off:** None for data worktrees — reuses the proven Lucidos Apply concurrency story. The `in_place` race is documented and rare.

### Q7. Should the compose-view repo dropdown be repurposed?

**Recommendation:** Repurpose as a "scope" picker. Top: Lucidos (default). Next: registered repos. Submenu: "App in this workspace" with `data/apps/*`. Plus a "custom path" entry that opens a folder picker.
**Trade-off:** Compose-view UI gets a bit more complex. The existing dropdown is a clean two-option list (Lucidos vs external repo). Adding a folder picker means dealing with arbitrary paths in the UI. Counterproposal: keep the dropdown for repos and add a separate "in this workspace" submenu wired to known data folders only, with the absolute-path case being LLM-only (no UI affordance). For v1, the LLM-only path is probably fine — users mostly trigger app edits by chat, not by clicking the repo dropdown.

### Q8. Folder inside `.lucidos/` (gitignored runtime cache)?

**Recommendation:** Refuse with a clear error. `.lucidos/` is ephemeral and rebuildable; editing it is almost always a bug.
**Trade-off:** None. Anyone with a legit reason to touch `.lucidos/` (engine developer debugging recovery) can pass the absolute path with a different mode flag if we ever need to escape the refusal.

### Q9. Folder = the whole workspace data root (`<ws>/data/`)?

**Recommendation:** Allow it. `worktree` mode wrapping the data git, cwd at the worktree root. Apply ff-merges to data git main. The user gets a coding-agent thread that can edit anything in the workspace and review-then-apply the whole batch.
**Trade-off:** Wide blast radius — but every file in the batch is visible at Apply time. The user can choose not to Apply and discard the worktree, which is exactly what the review gate is for. No structural objection.

### Q10. Folder = a subdir of Lucidos source (e.g. `crates/lucidos-engine/src/api/`)?

**Recommendation:** `worktree` mode wrapping the whole Lucidos repo; cwd at the subdir within the worktree.
**Trade-off:** CC's filesystem view includes the whole Lucidos worktree (it has to — Cargo needs the full tree). "Narrow scope" is intent-only — encoded in the system prompt — not enforced. CC may stray and edit files outside the requested folder. This matches today's behavior for `repo="Lucidos"` and is fine.

### Q11. Does the `SessionStarted` event grow, or do we introduce a new event?

**Recommendation:** Grow. Add `folder: Option<String>` and `mode: SessionMode` as additive fields with serde defaults. Don't create `FolderSessionStarted`.
**Trade-off:** The event keeps backward decoding for old rows. The alternative (new event type) doubles the consumer surface and complicates the projection. Keep it one event.

### Q12. Do we need a `mode` enum in the wire format, or can it be derived from `(branch, folder)`?

**Recommendation:** Store the enum explicitly. `mode = worktree` ⇔ `branch.is_some()` today, but a future mode (e.g. "live commits with no merge") might violate that, and the projection should not have to guess.
**Trade-off:** A redundant field that must stay in sync with `(branch, folder)`. Minor cost; clearer story.

### Q13. What does the harden marker do for data-tree worktree sessions?

**Recommendation:** Not used. `hardened_branches` is written and queried only for Lucidos worktree sessions. Data-tree Apply does not gate on a marker — the data `changes` row's `hardened` column is conceptually N/A and either left at its default `false` (with Apply ignoring it for data `repo_root`) or migrated to a tristate (`true` / `false` / `not_applicable`). Recommend the simpler path: keep the BOOLEAN column, change Apply's gate to `if repo_root == lucidos_dev_root && !hardened ⇒ run harden`.
**Trade-off:** No `STALE` detection for data worktrees. Acceptable — Lucidos's harden discipline doesn't apply to apps.

### Q14. The `lucidos-cli`/CLAUDE.md/skill files inside Lucidos worktree — what about inside data-tree worktrees and in_place folders?

**Recommendation:**
- **Data worktree:** the worktree wraps the data git, so anything tracked in the data git (e.g. an app's own `.claude/commands/harden.md`) is naturally available. The engine should NOT inject Lucidos's `.claude/commands/` — those reference Lucidos source layout and would mislead CC about which test commands to run. The system prompt explicitly tells CC "the Lucidos /harden skill does NOT apply here; use whatever this app provides."
- **In_place:** same — do not inject Lucidos skills. The app/folder provides its own.
- **No symlinking from Lucidos source.** The original design had a symlink option; it's not needed with the worktree-everywhere model.
**Trade-off:** CC working in a fresh app that doesn't ship any `.claude/commands/` won't have `/cpa`, `/loop`, etc. Acceptable — those are Lucidos-internal anyway.

### Q15. The `manage_repositories` tool — does it grow?

**Recommendation:** No. Repos remain the worktree-eligible list. If we later need folder discoverability, add `list_folders` as a sibling tool.
**Trade-off:** LLM has to know two different paths for two concepts. Probably fine; the existing tool description can point at the folder convention for in_place edits.

### Q16. Cross-workspace + relative folder path?

**Recommendation:** Allow `folder="data/apps/habit-tracker"` with `workspace="myws"`. The target resolves the relative path against its own root. Absolute paths are evaluated on the target's filesystem.
**Trade-off:** Slight ambiguity if the caller assumes the path is relative to the caller's workspace. Mitigation: the tool description must explicitly say "folder resolves on the target".

### Q17. What does the diff viewer show for a data-worktree thread vs an in_place thread?

**Recommendation:**
- **Data worktree:** identical to today's Lucidos diff viewer — list of files changed on the CC branch vs the data git's main, with the Apply button. Reuses `ChangesView.tsx` unchanged.
- **In_place:** a commit list — CC's commits during this session on the unregistered git's current branch — with no Apply button. Different UI from worktree threads because the concept (no staging gate) is different.
**Trade-off:** Adds the in_place variant of the diff viewer; data threads cost nothing because they reuse Lucidos's.

---

## 5. Tool signature diff

### Current `run_claude` schema (excerpt — see `crates/lucidos-engine/src/llm/tools/mod.rs:1346-1391`)

```json
{
  "type": "object",
  "properties": {
    "prompt":     { "type": "string", "description": "..." },
    "repo":       { "type": "string", "description": "Repository ID or name from manage_repositories — resolved in the target workspace's repo registry... REQUIRED whenever the work targets anything outside the Lucidos source tree... Omit ONLY when editing Lucidos itself." },
    "workspace":  { "type": "string", "description": "..." },
    "allowed_tools": { "type": "string", "description": "..." },
    "model":      { "type": "string", "description": "..." },
    "append_system_prompt": { "type": "string", "description": "..." },
    "images":     { "type": "array",  "items": {"type": "string"}, "description": "..." },
    "title":      { "type": "string", "description": "..." },
    "relation":   { "type": "string", "enum": ["child", "top"], "description": "..." }
  },
  "required": ["prompt"]
}
```

### Proposed `run_claude` schema (delta only)

```diff
   "properties": {
     "prompt":     { "type": "string", "description": "..." },
-    "repo":       { "type": "string", "description": "Repository ID or name from manage_repositories — resolved in the target workspace's repo registry... REQUIRED whenever the work targets anything outside the Lucidos source tree... Omit ONLY when editing Lucidos itself." },
+    "folder":     {
+      "type": "string",
+      "description": "Where Claude Code should run. Accepts (a) an absolute path like `/Users/.../workspaces/myws/data/apps/habit-tracker`, (b) a workspace-relative path like `data/apps/habit-tracker` (resolved against the target workspace's root), or (c) a registered repository name or UUID from `manage_repositories` (resolved to the repo's path). The engine picks the spawn mode automatically: folders inside the Lucidos source, the root of a registered external repo, OR anywhere under the target workspace's `data/` tree get an isolated git worktree on a fresh `claude-code/...` branch (with a pending-change row, an Apply gate that ff-merges to the enclosing git's main, and — for Lucidos source only — a `/harden` discipline before Apply). Folders inside an unregistered git outside those categories are edited in place (commits land on the current branch, no Apply gate). Folders outside any git are edited with no commits. Omit to edit Lucidos itself. Required whenever the work targets anything outside the Lucidos source tree — without it the session lands in the Lucidos worktree and edits go to the wrong place."
+    },
+    "repo":       {
+      "type": "string",
+      "description": "DEPRECATED — use `folder` instead. Accepted for one release as an alias: equivalent to passing `folder=<resolved repo path>`. Passing both `folder` and `repo` is an error."
+    },
     "workspace":  { "type": "string", "description": "..." },
     "allowed_tools": { "type": "string", "description": "..." },
     "model":      { "type": "string", "description": "..." },
     "append_system_prompt": { "type": "string", "description": "..." },
     "images":     { "type": "array",  "items": {"type": "string"}, "description": "..." },
     "title":      { "type": "string", "description": "..." },
     "relation":   { "type": "string", "enum": ["child", "top"], "description": "..." }
   },
   "required": ["prompt"]
```

No other field changes. `required` stays at `["prompt"]` (folder is optional; omitted means "edit Lucidos source").

The companion narrative in the tool *description* changes too: the "BEFORE CALLING" guidance is rewritten to talk about *what to edit* (Lucidos vs an external repo vs an app folder vs a data subdir) rather than *whether to set repo*. Not shown in the diff above — it's prose.

---

## 6. Risk list — what we're choosing to live with

1. **The workspace data git's branch history accumulates `claude-code/...` branches.** Every coding-agent thread on an app creates a feature branch on the data git. After many sessions, `git log --graph --all` on the data git is noisy with merged-and-deleted CC branches (post-Apply cleanup removes them, but reflog and `git log --all` retain the history). *Accepted* in exchange for the staging gate; the data git is rarely browsed manually.
2. **No engine-enforced harden discipline for app changes.** Apply on an app `ChangeProposed` clicks through without checking that the app's own tests passed. Per the user directive, apps own their hardening. A user who clicks Apply on a buggy app change has no engine-side safety net beyond CC's own bug-check pass during the session. *Accepted by directive*.
3. **`in_place` commit races on the unregistered git.** Concurrent in_place sessions on the same unregistered git can conflict at commit time. v1 ships without a mutex. CC surfaces the failure; the user retries. *Accepted, documented*. Less of a concern than the original design because in_place is the minority case.
4. **No path enforcement inside a worktree.** A session asked to edit `data/apps/habit-tracker/` can still write to `<worktree>/apps/anything-else/`. Mitigation: the Apply review surface shows every changed file, so accidental cross-app writes are visible BEFORE Apply rather than landing silently on data git main. *Accepted*.
5. **Cross-workspace trust is filesystem-based.** Same as today. A target workspace evaluates the folder against its own FS and trusts whatever the caller asked for. *Pre-existing, unchanged*.
6. **The `branch` field on `SessionStarted` becomes optional.** Downstream consumers (`MessageRoutePanel.tsx`, `thread_summaries` projection, the route-panel branch chip) need to handle absence for `in_place` and `no_git` threads. Code paths that today assume `branch.is_some()` need an audit. *Tracked work*.
7. **`thread_summaries.cc_repo_id` becomes ambiguous.** NULL today for Lucidos; NULL also for data-tree worktrees and in_place threads. The distinguisher is the new `cc_kind` column (`lucidos` / `data` / `external` / `in_place` / `no_git`). Consumers that read `cc_repo_id` to mean "is this an external-repo thread?" need to read `cc_kind` instead. *Tracked work*.
8. **Two distinct system prompts now mention worktree+Apply.** Today: `worktree_system_prompt` (Lucidos) + `external_repo_system_prompt`. Tomorrow: add `data_worktree_system_prompt` (worktree+Apply but no `/harden`, no engine restart). Plus an `in_place_system_prompt` for the rare path and a `no_git_system_prompt` for the no-git case. Four variants where today there are two. *Tracked work; the prose differences are real*.
9. **Unregistered git outside both Lucidos and the workspace data tree goes to `in_place` by default.** A user who wants worktree isolation for such a repo must register it. Surfaced in Q4. *Accepted*.
10. **No native LLM affordance for "list folders I can spawn against".** The LLM has to know the convention (`data/apps/*`) or the user must spell out the path. `manage_repositories` doesn't list app folders. *Accepted for v1; sibling `list_folders` tool later if needed*.
11. **The `cc_kind` distinguisher must stay in sync with `(cc_folder, cc_repo_id, cc_mode)`.** A redundant field that the projection populates from event payload. Drift would be a bug. *Tracked work; tested via the existing projection rebuild path*.
12. **The compose-view "scope" picker has more options than the old "repo" dropdown.** Adding `data/apps/*` (and possibly other data subdirs) to the dropdown makes the UI denser. The LLM-only path (absolute folder via tool args) keeps the UI from having to handle arbitrary paths. *Accepted*.
13. **`files_require_restart` must short-circuit on data `repo_root`.** Today the function scans path patterns and would return false for data files anyway (no `.rs`, no `Cargo.toml`, no `packages/lucidos-sdk/`) — but the policy should be explicit: skip the function entirely when `repo_root != lucidos_dev_root`. Avoids a future edit accidentally adding a pattern that matches a data path. *Tracked work*.

---

## Appendix: implementation-time documentation obligations

`.claude/rules/system-knowhow.md` enforces same-commit drift prevention: code changes to documented surfaces MUST update the matching `system-knowhow/*.md` and glossary entries in the same change. Before this design lands, the implementer must update each of the following:

### Glossary deltas

- `coding-agent thread` (in `system-knowhow/glossary.md`) — entry no longer states "scoped to a registered git repository". Update to: "scoped to a folder. The engine picks a spawn mode (worktree / in_place / no_git) based on where the folder sits — folders inside Lucidos source, a registered external repo, or the workspace's `data/` tree get a worktree on a fresh branch (with an Apply gate); folders in an unregistered git are edited in place; folders outside any git are edited with no commits."
- `worktree` (in `docs/glossary.md`) — entry stays accurate; add the data-git case to the examples.
- **NEW** `session mode` or `folder mode` (in `docs/glossary.md`) — enum: `worktree`, `in_place`, `no_git`. Defines which spawn path the engine takes.
- **NEW** `session kind` (in `docs/glossary.md`) — enum for worktree-mode sessions: `lucidos`, `data`, `external`. Determines Apply behavior (ff-merge with restart check / ff-merge no restart / no Apply) and harden discipline (Lucidos full / none / none).
- **NEW** `data worktree` (in `system-knowhow/glossary.md` if user-facing) — a coding-agent thread on a folder inside the workspace's `data/` tree. Same machinery as a Lucidos source coding-agent thread (worktree, branch, change, Apply, ff-merge) but on the workspace's own data git; no engine restart and no Lucidos `/harden` discipline.

### `system-knowhow/*.md` updates

- `system-knowhow/thread-events.md` — the `SessionStarted` row at line 100 currently reads "Carries `session_id` (CC's CLI session id), optional `branch`, optional `repo_id`." Must be updated to reflect the new payload (additional `folder: Option<String>` and `mode: SessionMode`; `branch` truly optional). Triggered by the rule row "`ThreadEvent` enum — variant added/removed/renamed, payload field changed" → `system-knowhow/thread-events.md`. Also: the `ChangeProposed` documentation must note that `repo_root` can be a workspace data git (not just Lucidos source) and that the `hardened` semantics differ by `repo_root`.
- `system-knowhow/coding-agent-events.md` — re-check at implementation time. The file does not currently mention `SessionStarted` (grep confirms). Any narrative there about "the worktree wraps the Lucidos source" must be generalized to "the worktree wraps the enclosing git (Lucidos source, a registered external repo, or the workspace data git)".
- `system-knowhow/building-an-app.md` — the user-facing app authoring doc must say "coding-agent threads on apps go through a worktree + Apply gate; quick chat edits commit directly". The Apply ergonomics for app changes is new user-visible behavior.
- `.claude/rules/db.md` § Key event types — `SessionStarted` row notes need updating; the `thread_summaries` row needs the new `cc_folder`, `cc_mode`, `cc_kind` columns; the `changes` row needs the note that `repo_root` can be a data git.

### Other doc surfaces touched

- The `run_claude` tool *description* (prose, not just JSONSchema) in `crates/lucidos-engine/src/llm/tools/mod.rs:1346-1391` — rewritten per §5.
- The engine system prompts at `crates/lucidos-engine/src/engine/agent_session/prompts.rs` — needs **two new variants**: `data_worktree_system_prompt` (worktree+Apply, no `/harden`, no engine restart language, no `scripts/web-dev.sh` references) and `no_git_system_prompt` (warn that nothing is tracked). The existing `worktree_system_prompt` stays Lucidos-only; rename it to `lucidos_worktree_system_prompt` for clarity. The selection logic at `run_session.rs:235-252` grows a four-way branch.
- `system-knowhow/best-practices.md` and `system-knowhow/intent-registry.md` — if either advertises `run_claude` usage by repo, the wording needs updating to point at `folder`.

If any of these is missed at implementation time, `/harden` will flag it as a hardening failure per the system-knowhow rule.
