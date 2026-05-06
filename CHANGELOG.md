# Changelog

## v0.9.1 — 2026-05-06

### Added
- **Export thread** button next to "Copy reference" in the thread header.
- Copy button on thread headers reveals on hover (desktop).
- CC commands menu scopes compose-view skills to the selected repo.

### Fixed
- **Compose**: textarea sync skipped while user is focused (cursor jump on remote draft updates); persisted draft text now reaches the focused textarea on reload; `focusedThreadId` persisted across reloads; `applyRemoteCompose` guarded against orphan + empty drafts; send routed via `composeMode` instead of the draft's frozen channel; draft state split cleanly between local + remote.
- **Claude Code**: parallel tool calls with the same label no longer get misrouted (was causing UI spinning).
- **Thread lifecycle**: stale-resume guard in `run_session.rs` so transient API errors no longer destroy user work via branch deletion; threads with pending changes pinned to Review before archiving; `ChangeProposed` moved to a CC-specific unread-marking arm; `pendingChanges` counter tracks `ChangeProposed`/`ChangeApplied`/`ChangeDiscarded` correctly.
- **Memory module**: artifact-indexing gaps causing some artifacts to be missed in long-term memory.
- **Settings**: `openSettingsSubview` clears `panelOverlay` and removes `file-preview-open` / `app-window-open` from localStorage.
- **Release wrapper**: `scripts/release.sh` no-PR mode now bases the worktree off `main` instead of the previous tag, so local commits actually ship (this was the v0.9.0 regression).
- localStorage v5 hash bug.
- Merge cancellation bug (verified by 3 new tests in `dismiss-requesting.test.ts`).
- CCSettingsChanged thread lifecycle violation on CC sessions started with non-default settings.

### Changed
- Thread state machine clarified into `composing` / `active` / `discarded` / `archived` (engine/thread_state.rs).
## v0.7.1 — 2026-04-27

### Highlights
- **CC Resume Architecture rewrite** — process exits on every idle (including permission prompts), resumes cleanly on user input, and reconstructs context when the prior session is stale. Replaces the always-alive CC loop; resume-by-id (`--resume <session_id>`) is still the mechanism for normal resumes, but the user-answer flow now always starts fresh and reconstructs context instead of resuming by id.

### Added
- **CC resume / coding-agent lifecycle**
  - Process-exits-on-idle model with reconstruct-on-resume; "Cancel" treated as a resumable turn boundary.
  - `AskUserQuestion` now routed via a `PreToolUse` hook + `/api/internal/ask-user-question` long-poll endpoint (replaces the brittle reconstruct path).
  - Detects external edits and branch drift on resume; catches worktree up to `main` automatically.
  - Coalesces rapid-fire idle messages into a single resume spawn.
  - Reconstructs conversation summary on stale resume; preserves worktree CWD across `AskUserQuestion` resume.
  - Renamed user-facing label from "Session resumed" → "Thread auto-resumed".
- **Trigger pause/resume** — `pause_trigger` / `resume_trigger` LLM tools and a `paused` field on `update_trigger`.
- **Mode-driven actor chips & Engine Explainer popover** — initiator panels show the right "[icon] WHO — WHAT" with proper device labels and engine-vs-agent attribution.
- **`lucidos data-store add` CLI** — move directories into `~/.lucidos/data/` for persistent bulk reference corpora outside the workspace.
- **Free-disk-driven worktree cleanup** — replaces the hardcoded 50 GB cap with live volume monitoring; stacked disk-usage bar + per-row % of worktree usage; dedup'd disk-low alerts with auto-cleanup notifications; new `GET /api/disk-usage/summary` endpoint.
- **Rendered/source toggle for markdown change diffs** (`bf804d8f` + harden) — preview rendered markdown alongside raw diff in the changes panel.
- **Bulk-import size guard** — `git_clone` and file uploads refuse >500 files / >100 MB into `data/artifacts/` with a pointer to `.lucidos/tmp/` or `~/.lucidos/data/`.
- **Versioning polish** — Client version row in Tauri + web; Lucidos version asterisked when there are commits since the last release.
- **CodingAgent (CC) pluggable refactor — Phase 1** (per `docs/plans/2026-04-20-cc-to-coding-agent-refactor.md`).

### Changed
- **Harden state is DB-backed** — Phase 0 and `pre-push.sh` consult `lucidos hardened query` instead of filesystem markers.
- **Knowhow injection moved to execution time** — engine concatenates formatted knowhow onto intent text in `scheduler/user_tasks.rs` / `core/knowhow.rs`.
- **`/api/v1/data` no longer exposes system-docs** (`list_data`, `read_data`, `validate_data_path`).
- **CC follow-ups use auto-resume via `chat_submit`** — removed the dedicated CC idle waiting loop and the silent-CC-auto-resume-on-focus path.
- **Major refactors** — `engine/claude_code.rs` (5,804 lines) split into `engine/cc/`; `engine/cc_runtime.rs` (3,098 lines) split into 10 sub-files (`spawn`, `io_helpers`, `parsing`, `lifecycle`, `runtime_helpers`, `prompts`, `resume`, `apply_now`, `run_session`).

### Fixed
- **CC panic-on-resume** — answering a question on a trigger thread no longer panics; resume path dropped from the user-answer flow (`762fd6fb`); always start fresh and reconstruct context.
- **Stale resume edge cases** — don't delete branch when discarding stale change; `SessionEnded(stale_resume)` no longer changes thread status; `stale_resume` added to `NORMAL_SESSION_END_REASONS`; clean up worktree on stale CC resume.
- **Resume detection** — detect idled-then-resumed CC sessions as actively running; resume actively running sessions even with a pending change; resume running CC sessions after restart even without git changes.
- **Resume race conditions** — emit `MessageReceived` before routing CC/chat follow-ups; emit `ClaudeCodePromptSent` when CC resumes while waiting; prevent SSE race, debounce resume, re-fetch on empty thread; resolve model/effort before `spawn_or_resume`; skip `ResponseGenerated` during silent resume.
- **Apply path** — reuse existing CC worktree on locked branch (`73dce58e`); surface `timestamp out of range` and `worktree git_ops` errors that were leaving changes pending behind a 200 OK.
- **Message attribution** — spawned threads and child→parent callbacks were labeled "Lucidos Engine" instead of "Lucidos Agent"; fixed across `agentic_loop`, `claude_code`, and thread-events paths (`ParentThread` origin stamped on `run_thread` / `run_claude` child messages).
- **Threads stuck in `running`** — `get_recent_threads()` was dropping threads with pending changes due to a 15-row partition limit.
- **Image-only messages froze threads** — sending an image with no text no longer leaves the thread at `running`.
- **Large image payloads** — strip `[IMAGE_CONTENT:]` markers at write-time (`agentic_loop.rs`) and read-time (`/api/threads/{id}/events`, ToolResult payloads).
- **Interrupting a spawned chat thread** now cancels model/tool calls and transitions the thread to `aborted`.
- **Diff rendering** — restore diff context after reload via path-encoded `changeId`; merge adjacent strips; persist source-toggle scoped to reload only; full-width bg highlight covers margin gaps; track lines by actual newline count.
- **Mobile** — keep pin button tappable while keyboard active; elevate `.thread-content` above `.edge-swipe-zone`; reverted brittle `contain` isolation in favor of memoized linkify; preserve mobile header in scale modal; long thread titles wrap in disk-usage view.
- **Performance** — memoize response-text `linkifyPaths` to fix swipe jank (`eb3dfa91`); batched path regex to dodge WebKit "regex too large"; stable empty-array fallbacks in chat.
- **iOS Safari Service Worker `TypeError`** in `crates/lucidos-app/public/sw.js`.
- **`run_claude` origin** — replaced raw struct construction with `make_message_received` in `claude_code.rs`.
- **Permission card** — right-align actions, Allow rightmost.

### Tests
- New coverage for the CC resume rewrite: process-exits-on-idle lifecycle, `AskUserQuestion` PreToolUse hook + long-poll endpoint, stale-resume / branch-drift / external-edit detection, and reconstruct-on-resume context building.
- Effort/model persistence + `opus[1m]` case (`crates/cognos-app/src/components/chat/__tests__/cc-reasoning-effort-persistence.test.ts`, 12 tests).
- Mode-driven actor chip + Engine Explainer popover coverage (backend attribution + frontend rendering).
- Trigger pause/resume tools and `paused` field on `update_trigger`.
- Image-only message handling and `[IMAGE_CONTENT:]` strip at write-time and read-time.
- Free-disk-driven worktree cleanup and `/api/disk-usage/summary`.
