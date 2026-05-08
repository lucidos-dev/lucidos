# Changelog

## Unreleased

### Added
- **Proxy auth modes** (`apis.json`):
  - `query_param` — inject the credential as a URL query parameter (`?api-key=…`); useful for Helius / Solana RPC.
  - `hmac_signed` — sign each request with HMAC-SHA256 / SHA512 over the query string (with optional millis-since-epoch timestamp injection); Binance shape.
  - `credential_bundle` — return a JSON map of credentials over `GET /api/v1/proxy-credentials/<name>` (CLI: `lucidos proxy <name> --credentials`) for libraries that perform their own login (e.g. `pcomfortcloud`). The `proxy_request` LLM tool refuses this mode so raw credentials never reach the model.

### Changed
- `ProxyAuth` is now a serde-tagged enum (one variant per auth mode). The on-disk shape for `bearer` / `api_key` / `basic` is unchanged — existing `apis.json` files keep working.

## v0.9.4 — 2026-05-08

### Added
- **Content-addressed image blob store** — backend foundation, one-shot startup migration of legacy base64 payloads, `image_hashes` over the chat HTTP and event payloads, frontend uploads-on-attach with blob-URL preview, downscaled iOS-Safari preview endpoint, EXIF handling.
- **Image popup navigation** — single-finger mobile swipe between images, desktop chevrons pinned to viewport, smooth swipe-commit, prev/current/next slot rendering for true carousel feel.
- **Save / Archive prompt actions** — Save button on Active threads, ✓ Saved unsave toggle on running Saved threads, section-aware Save/Archive in prompt area, collapsed Active+Saved action area, "Thread saved" toast, smooth mount/unmount fades.
- **API proxy auth modes** — `query_param`, `hmac_signed` (Binance-shape signing), and `credential_bundle` (with `/api/v1/proxy-credentials` endpoint and `lucidos proxy <name> --credentials` CLI). `proxy_request` LLM tool refuses `credential_bundle` for safety.
- **Plugin install via chat drag-and-drop** — drop a `.lucidos-plugin` archive into the chat to install.
- **CC AskUserQuestion nudge** — system prompt nudges CC to use `AskUserQuestion` for choice-shaped questions.
- **Real input-token counts in the thinking chip** — uses provider usage instead of estimates, applied to inline chip too.
- **CLI `spawn-thread`** — renamed from `send-thread`, added `--repo` selector.
- **Title generation keeps ticket/issue identifiers** — preserves identifiers like `ABC-123` in generated thread titles.
- **Restarting-engine toast** — non-dismissable flag, hides the X on the restart toast.

### Changed
- **Mobile copy-button** — bigger tap area on copyable + code blocks, ::before tap area, snap-aligned with `.action-btn` pattern.
- **Code-block header** — collapsed into one overlay row, copy button visible on touch devices, hover-reveal restored on desktop, isolated from scroll via inner wrapper.
- **Send button morphs into Cancel** — single morph instead of unmount/remount; `--duration-emphasis` token; 500ms fade so the color change is perceptible.
- **Image popup nav buttons** — darker close/delete buttons.
- **Mobile cold launch** — active pane scoped to sessionStorage so cold launch lands on the focused thread.
- **iOS PWA navigation** — app switches no longer pollute session history.
- **Knowhow id resolution** — app-scoped ids (`<app>/<rest>`) resolve in trigger validator; absolute `rest` blocked from path escape; trigger knowhow ids validated end-to-end.
- **Internal: `ProxyAuth` is now a tagged enum**; credential lookup consolidated; `query_param` credentials redacted from logs.

### Fixed
- **Image popup** — pinch-zoom no longer thrashes layout, gesture conflicts cleared, swipe flicker removed (decode-before-swap), pending swipe-commit flushed on follow-up gesture, gesture lock released on pinch end, stale pinch rAF dropped before swipe.
- **Chat send / cancel flow** — Send disabled while CC awaits a question/permission; awaiting-answer gate closes during send→SSE round-trip; prompt re-enabled while CC awaits a question; mode toggle allowed on composing threads before first send.
- **Chat scroll** — observers gated on element visibility; `awayFromBottom` reconciled on scroll-to-bottom loop exit; escalated on resize so panel-expand shows the chevron; pinned to bottom when answering a CC question.
- **Save/Archive UI** — hidden mid-turn and when Apply is pending; double Archive button on saved-thread prompt removed; Save no longer flashes before Send when only pending image uploads are content; `✓ Saved` toggle always shown on Saved threads; right-aligned lone section button.
- **Image popup overlay** — covers header so controls are disabled during preview.
- **Restart overlay** — covers header so controls are disabled during restart.
- **Drawer exchange count** — stabilized by trusting server `messageCount`; inlined helper.
- **Plugins** — per-route body cap; tighter sanitize; upload logs; tokio::fs upload; shared `PLUGIN_ARCHIVE_EXT` constant.
- **Title generation** — emits `"Image"` / `"Images"` instead of LLM-hallucinating on empty input.
- **Changes panel** — already-applied branches treated as no-op instead of erroring.
- **CC** — toast suppression for "Failed to load CC commands" during engine restart; `engineRestarting` refetch effect scoped to compose view; never surfaces "success" as `ResponseFailed.error`; tags partial-run changes.
- **File preview** — knowhow-list fetch errors surfaced instead of swallowed.
- **Thread title** — re-fits display height when container width changes.
- **Recovery** — reuses deterministic worktree path when rebuilding lost CC sessions; clears stale dir before `worktree_add`.
- **Blobs** — `resolve_thread_image_refs` surfaces missing-blob with a clear error; `thread:N` references stay stable; dropped hashes logged; bad-encoding distinguished; in-memory blob URL kept alive for confirmed previews; preload server URL before swapping preview src; per-call nonce in preview tmp filename; `ImageUploaded` added to `thread_lifecycle` section-transition allowlist; compose-image migration doesn't wipe already-migrated drafts.
- **Mobile** — keyboard state preserved across image picker; image picker keyboard-restore reverted (iOS won't honor it).
- **Diff** — always offered for external-repo CC threads.
- **Header** — pointer cursor only on visible brand children; full-width dblclick on brand-label empty space allowed; brand-area toggle skips full-width dblclick; tooltip dropped when it just repeats the visible label.
- **Tooltip** — `cursor:help` dropped; redundant tooltips suppressed in the global system; `currentTarget` cleared on suppression.
- **Theme** — `matchMedia` change listener re-enabled off iOS.
- **Releases tooling** — ancestry check tightened (HEAD, surface git errors); local main fast-forwarded after Mode 1 release.

### Removed
- Stale narrative comments and historical SHA references across chat, blobs, recovery, plugins, image popup, drawer, and proxy modules (project-wide harden + simplify passes).
## v0.9.3 — 2026-05-06

### Fixed
- Lucidos Agent icon: swapped 🤖 robot for ✨ sparkles in initiator and executor chips
- Mobile send: action button now blurs on click instead of pointerdown, so the keyboard no longer eats the tap

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
