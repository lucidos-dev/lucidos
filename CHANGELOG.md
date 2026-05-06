# Changelog

## v0.9.0 — 2026-05-06

### Added — Plugins
- **Plugin system v1** — install / update / uninstall workspace bundles (apps, knowhow, triggers, scripts) from GitHub tree URLs, plain git URLs, or local `.lucidos-plugin` archives
- Manifest schema, semver-aware updates, conflict detection on overwrite
- v1 guide-only uninstall (emits PluginUninstalled, lists files, no auto-delete)
- `browser-learning` shipped as the first reference plugin
- Authoring docs: `system-knowhow/building-a-plugin`
- Event triggers can ship in plugins; cron triggers cannot (install-time UX guidance)

### Added — Section reorganization & navigation
- Restructured panels: Files / Apps / Triggers / Settings / Changes / Notifications
- Dedicated **pending Changes** panel with per-change apply/discard + Review routing
- **Drafts** section in thread drawer; drafts in back/forward stack; nav restores cursor entry
- Browser-style back/forward navigation in panel header
- `cognos.navigate()` API for app iframe navigation; `'thread'` as a nav target
- File search modal in Files panel (with keyboard nav)
- `.slides` file preview in file preview panel
- `t` shortcut toggles threads panel; Cmd+↑ dismisses UI scale panel
- Edge swipe zones for pane navigation over iframes
- Arrow-key nav in thread drawer and CC Commands dropdown
- Grid icon to jump to ContentPane from mobile thread view
- Connect Account card in OAuth section of Accounts settings
- Aborted banner panel; DisplaySection::Waiting for threads with active children
- Diff button on WaitingBanner for quick change preview
- Header divider; CognOS brand merged into panel title when collapsed
- Code review gate for worktree-to-main merges

### Added — Engine / runtime
- **Generic API proxy** for HTTPS-iframe apps (auth + path-traversal hardening) — now the preferred way to call external APIs
- **Gzip-compressed SSE event stream** (honors `q=0` in Accept-Encoding)
- **ThreadAggregate snapshots** on persisted thread events; frontend consumes from SSE with lookup fallback
- Mid-flight injection split into separate exchange at UPI boundary
- CC compose-view skills scoped to selected repo
- Toasts get close X; robot icon for Lucidos Agent

### Changed
- Trigger fire prompt split into system addendum + user header (cache prefix sharing)
- Fanout: parent-update consolidated into one round-trip
- SSE encoder: no per-event String allocations
- MAX_FACTS cap dropped 60 → 25
- README rewrites: companion framing, intent + automation, no-build-step, autonomy clause
- Knowhow: API proxy as preferred external-API path; lucidos CLI output streams (`--include` is stdout, `--fail` to stderr)
- `building-a-trigger` knowhow: always confirm review-vs-history surface

### Fixed
- **Archive**: only cancel live CC subprocess; wait for cancel-fallout terminal event before ThreadArchived; double-click bug; ResponseCanceled's ThreadMarkedUnread side effect
- **Compose**: pendingComposePuts lifecycle; focus/nav release on peer ThreadDiscarded; in-flight keystrokes preserved across loadAllThreads; whitespace-only treated as empty; mark-pending at schedule time; cursor preserved on same-thread re-syncs; suppress SSE clear when MessageReceived/ThreadDiscarded came from this device
- **Chat**: Continued-below panel hidden for empty/Thinking-only events and 'interrupted' status; lazy-fetch Change for old applied IDs; ChangeBody loading state; clearer unsave dialog copy; widened Saved button; trailing thread metadata no longer flips absorbed-UPI to 'aborted'
- **Prompt**: iOS PWA photo picker selection attaches to draft; restored 0.7.2 file input pattern; hidden file input wrapped in `<label>`; visually-hidden shared class; action buttons no longer raise mobile keyboard; restore composeHandlers on wide-path photo button
- **CC**: route tool result via tool_use_id; suppress SessionRecovered for answered_after_idle; don't treat error result as stale resume; ResponseFailed on mid-stream API error
- **Engine / orchestrator**: emit Aborted (not Canceled) on stuck-thread eviction; preserve tool-call memory across parent-callback resumes; stale-resume guard requires `cc_error.is_none()`; event-bus decrements parent active_children_count when CC child canceled/aborted; harden propagates initiator parse error
- **Memory**: artifact-indexing gaps closed; opt-in re-extract for stale entries
- **Backup**: exclude `data/postgres.*/` archives from tar; clear backupProgress on SSE reconnect
- **Theme**: drop matchMedia 'change' listener (iOS PWA flash root cause)
- **Diff**: filter all engine-injected paths from user-facing diffs
- **`edit_file`**: support quoted keys + JSONPath root in `json_path`
- **Apps**: don't auto-open when files are touched
- **Drafts**: never render composing rows with no text and no images
- **Thread-nav**: Back from compose mode restores cursor entry; clear nav entry when startCompose POST fails
- **Thread-title**: ellipsis when truncating preview at 40 chars
- **Repo**: always flash loading in loadRepoFiles
- **Exchange-status**: empty non-last chat exchange reads as 'done'
- **e2e**: kill orphan iOS Simulator before runs; gate VM pkill on CoreSimulatorService
- Project-wide harden pass

### Removed
- `ThreadMarkedRead` / `ThreadMarkedUnread` events + unread field
- `docs/issues.md` (migrated to workspace tracker)
## v0.7.2 — 2026-05-04

Bugfixes.

### Fixed
- **(#1, Akram)** History infinite-scroll no longer fires when the History section is collapsed. Previously, collapsing shrank the list and pulled the `IntersectionObserver` sentinel into view, silently appending ~15 threads (and inflating the count badge) on every toggle.
- **(#1, Akram)** New-thread shortcut tooltip now shows `⌃⇧O` on Mac. The old `⌘⇧O or C` was misleading: Cmd+Shift+O is intercepted by the browser/OS on Mac (only Ctrl+Shift+O actually fires), and `or C` was easily misread as `or Cmd+Shift+C`. The standalone `C` shortcut still works in code, just no longer advertised.
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
