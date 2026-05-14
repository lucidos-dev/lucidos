# Changelog

## v0.9.6 — 2026-05-14

### Fixed
- **Triggers:** only human `MessageReceived` events promote a trigger thread to REVIEW; engine-driven follow-ups no longer falsely resurrect/re-route trigger threads (event_bus_projection now filters by `mode = human`).
- **Title generation:** reject LLM titles that echo the system instruction back as the title.
- **Commit hook:** match engine TLS scheme so per-commit `ChangeProposed` fires reliably (was using wrong scheme on HTTPS-only engines).
- **Release script:** harden `-c FILE` mode and the deleted-files drift check; use `printf` instead of `echo` for grep input to avoid word-splitting on changelog bullets.
- **E2E:** rename test `event_type` `SessionRecovered` → `ContinuationStarted` to match the post-v0.9.5 event rename.

### Changed
- **Cleanup:** complete `app_id` rebind cleanup; lift dirty check out of merged arm.
- **Harden:** project-wide harden pass; tighten projection SQL comment around `ActorMode::Human`; drop commit-SHA references from title echo-validator comments.
## v0.9.5 — 2026-05-13

### Added
- **Pluggable proxy auth pipeline + WASM signers** — `apis.json` migrated from a 6-variant `ProxyAuth` enum to a `Vec<AuthLayer>` pipeline (`static_credential`, `script_handshake`, `hmac_signed`, `wasm_signer`); same-host redirect re-signing; cross-host refused with 502; 1MB body threshold + manifest-declared `body_mode`; pipeline-aware 401 retry across opted-in cache hits; `WasmSignerLayer` with sign-only ABI, `SignInput`/`SignOutput`, capability gating, host imports for crypto + opaque secret handles, module-loader sidecars; first-class **Binance HMAC** signer; `reload_proxy_modules` LLM tool + HTTP endpoint; plugins can ship signers via `auth-modules/`.
- **`script_handshake` proxy auth** — token cache with singleflight gate, retry on cached-token 401; OAuth tokens injected into handshake env; `script_handshake` follower no longer flags `cache_was_hit`; replaces `credential_bundle`.
- **Background bash trio** — `run_bash_background` / `bash_output` / `bash_kill` chat tools backed by a new `BackgroundBashRegistry`; `BackgroundBashStarted/Completed` thread events wired through the lifecycle.
- **`ContextSnapshot` event + unified context modal** — per-LLM-iteration snapshot with sections + real provider usage; frontend collapses Step + Context tabs into one `ContextSnapshot` panel; estimated token count surfaced; legacy `ContextTokensMeasured` / `ContextAssembled` / `Thinking` events deleted.
- **Typed `ChildThreadCompleted` + child-completion card** — exchange-starter event with status, summary, link, disclosure; auto-resume callback for sub-threads; `dismiss_from_context` tool to drop prior tool results / child completions from resume context.
- **`run_thread` / `run_claude` `relation: sub|top`** — sub spawns auto-resume parent; top is fire-and-forget; CLI gains `--relation sub|top` on `spawn-thread` (replaces `--parent`); typed `top`-relation pathway through `notify_parent_if_child`.
- **Multi-select `AskUserQuestion`** — `AnswerKind::MultiSelected`, `multi_select` flag on `UserQuestionAsked`, multi-select toggle + Submit in the prompt action row; CC hook joins selected labels; option-id + compatibility validation.
- **CC stop-hook plaintext-question redirect** — detects plaintext questions in the CC transcript and redirects them through `AskUserQuestion`; UUID sentinel path; question-redirect reason text.
- **Per-trigger knowhow** — `data/triggers/{slug}/knowhow/`; LLM uses `load_knowhow` like chat instead of inlined preload turns; end-to-end ID validation across core, HTTP, LLM tools, and the scheduler.
- **Plugin uninstall + lifecycle** — real uninstall with `PluginUninstallPanel` confirm UI, deletes recorded files, stamps actor on confirm/cancel; `uninstall_plugin` resolves by id, name, or installed app folder; install-via-chat drag-and-drop; `delete_file` refuses plugin-owned paths; refresh apps + triggers on `Plugin{Installed,Uninstalled}`; install-state keyed by canonical plugin id.
- **`lucidos.ui.startThread` SDK API** — prefilled new-chat from app code.
- **Step detail modal** — clickable CC step rows on desktop, hover/tap tooltip on mobile, event timestamp; renders TodoWrite todos.
- **Notification → originating thread** — notifications link back to the thread that spawned them; standard 0.5rem gap between detail action buttons.
- **Permission card answer state** — keeps prompt buttons after answer with picked/struck styling.
- **Vertex prompt caching** — caches tools, system, and conversation prefix on Claude requests.
- **CC nightly-pipeline + run-tests + run-e2e skills** — per-batch CC orchestration recipes.
- **Image popup wraparound + slot rendering** — true carousel feel, n=2 black flash on swipe fixed, tap toggles chrome.
- **Code-block ellipsis highlighting** — visible elision in tool descriptions.
- **Tooltip-on-scroll + capture-phase listeners** — open tooltips follow target on scroll, passive capture for global scroll/touch.
- **Capture-context settings toggle** — opt-in deletion of unused `saved_contexts`.
- **Files panel surfaces `config/` + `auth-modules/`**.
- **Restart toast Dismiss action** — hides until a new change arrives, JSON fingerprint excluding engine version.
- **`/app/<id>/` route move** — app UI routes off `/api/`.

### Changed
- **System-knowhow doc set expanded** — `system-knowhow/coding-agent-events`, `system-knowhow/thread-events`, `system-knowhow/intent-registry`, `system-knowhow/workspace-audit`, `system-knowhow/workspace-learning`, rewritten `system-knowhow/building-an-auth-handshake` for the pipeline + WASM signer architecture; `building-a-trigger` rewritten for the post-preload model; tools docs clarified that `run_claude` is same-workspace only and that `run_bash` `timeout_secs` should be bumped for long jobs.
- **`read_file` archive support** — line-range slicing + transparent zip traversal; tighter `validate_archive_entry_path`; clear message for binary entries inside archives; zip-entry decompression capped at 10MB; schema mins on line args; deduped extension sniff.
- **CC stop-reminder hook** — plaintext-question detection, `LUCIDOS_SESSION_KIND=interactive` for chat-style CC sessions, `transcript_path` parsed from hook payload, sentinel-write failures surfaced on stderr; CC PreToolUse coerces Read offset/limit and forces Read-before-Edit.
- **CC Bash kill-pattern guard** — blocks kill patterns that would catch sibling CC subprocesses.
- **Typed `CancelCause` / `AbortCause` on `Response{Canceled,Aborted}`** — emit centralized through helpers; stale-settle moved from `CancelCause` to `AbortCause` (idle status, no Continue surfacing).
- **`SessionRecovered` → `ContinuationStarted`** — rename + lifecycle violation fixed; duplicate restart abort suppressed.
- **`grep_files` capping** — per-line and total result size capped to prevent context overflow.
- **Credential UX** — copy buttons for all credential types; rows wrap at narrow widths; LLM asked for one credential at a time; mobile autofocus skipped.
- **CC banner Diff button** — always rendered, disabled when no signal; lifted Save into Diff row when Apply gains the "& Restart" suffix; merged Diff into actions row when there's room.
- **Scheduler refactor** — backup pipeline extracted to `backup.rs`; task runner free fns extracted to `task_runner.rs`; tighter visibility.
- **Inline tests lifted to sibling files** across plugins, threads, change_ops, claude_code, agentic_loop, agent_recovery, changes_projection, thread_events, chat/process, engine mod.rs/run_session.rs, memory, llm/tools, email, change_ops, event_bus.
- **Project-wide harden + simplify passes** — narrative comments trimmed; helper extractions; redundant guards dropped; `responseCanceledSummary` JSDoc tightened; many small DRY wins.
- **Workspace data walker now includes `scripts/`**.

### Fixed
- **Postgres password leak through Bash tool calls** — redactor short-circuited on no-match; PG env bundle cached.
- **Orphan `tool_use` repair** — single source of truth + tighter validator; engine LLM repairs orphan blocks before they reach Anthropic.
- **CC phantom `ResponseCanceled`** — stopped emitting on Apply/Discard/Archive/idle and on conflict-resolution session ends; safety-net firings treated as crashes (error state, no `ChangeProposed`).
- **Engine cancel mid-tool-execution** — chat honors cancel; SIGKILL hung subprocesses; `emit_response_canceled` made idempotent against pre-emitted terminators.
- **System-actor activity events no longer resurrect terminated threads** (projection fix).
- **`changes_projection` flake** — cutoff/order tests de-flaked; constant for cutoff gap.
- **Apply ordering** — serialize against concurrent data writes; `delete_file` locked against apply dirty check; helper method on `workspace_repo_lock`; gate apply on real marker, not session-end.
- **Backup** — Google Drive resumable upload protocol; O(1) chunk-body clone; deduped Drive PUT helpers.
- **Wasmtime test isolation** — own binary to avoid macOS Mach IPC abort; shared engine between WASM compile + instantiate; loader returns empty for missing dir.
- **CC questions** — orphaned `UserQuestionAsked` skipped during pending-question lookup; archive still cancel-stamps orphaned questions; orphan-of-orphan re-process; frontend `req_id` routing.
- **Title generation** — skip opaque IDs (UUIDs, hashes), reject empty LLM responses, reject titles that echo the prompt instruction, instruction moved to system prompt.
- **Compose race** — await thread-create before pasting an image; await thread-start POST before debounced compose PUT; `pendingComposePuts` leak plugged; PUT skipped on discarded thread.
- **Drawer pagination** — gated on `archive` (renamed from `history` since v0.7.2); regression tests tightened.
- **`HEAD` is current** — re-applied lost Akram fixes (Ctrl+Shift+O on Mac, history-collapsed pagination guard) with regression tests.
- **`ResponseFailed` on empty CC Result text** — surfaced explicitly.
- **CC context** — sums `input + cache_read + cache_write` for total prompt size.
- **Stale `apply` timeouts** — extended so backend doesn't outlive client `AbortController`.
- **File preview download attribute** — uses real basename; deduped basename derivation.
- **Permission card** — Allowed badge right-aligned on resolved card.
- **z-index** — `--z-modal` lifted above `--z-control-panel` so modals block the header; `.toast-container` routed through `--z-toast` token.
- **iOS** — shake-to-undo blocked from wiping focused input (then reverted as iOS popup is system UI); landscape allowed when image popup is open.
- **Worktree cleanup** — skips threads with live agent session; deterministic stale-dir reuse on lost-session recovery; chat repo resolver narrowed to names only; `repo_root` fallback when default Lucidos row missing.
- **Image popup** — gesture lock release on pinch end, lock-before-flush; cancel commit timer when pinch starts; flush pending swipe-commit on follow-up gesture; render every image at fixed slot, signal-driven transform.
- **Plugin install** — per-route body cap; install sentinel redacted from LLM; confirm panels closed in `finally` so failures don't wedge UI.
- **`refreshChangesState`** — retried once on `AbortError`.
- **Email + OAuth row deserialization** — `sqlx::FromRow` derive replaces hand-rolled impls.
- **`throwIfNotOk`** — falls through to `statusText` when JSON has no error field; mutating handlers routed through it.
- **Triggers UI** — Delete/Edit kept at full opacity when paused; dead `trigger-toggle-btn` class dropped.
- **Header** — only opens control panel when clicking visible brand elements.
- **Notifications** — `optional chain` restored on `detail.thread_id`.
- **Tools** — `generate_image` misuse guard (warn when called for analysis instead of synthesis); type-driven tool dispatch.
- **Release wrapper** — refuses Mode 1 release while Mode 2 PRs are unmerged to main; deleted-files-vs-PREV_TAG drift check; `--accept-drift` escape hatch.

### Removed
- Six-variant legacy `ProxyAuth` enum (single-release migration to pipeline).
- `credential_bundle` proxy auth (superseded by `script_handshake`).
- Legacy `ContextTokensMeasured` / `ContextAssembled` / `Thinking`-tokens events (replaced by `ContextSnapshot`).
- `wake_text` from `ParentCallback` (typed event is the source of truth).
- Skill: `run-nightly-pipeline` (Lucidos territory, not a CC skill).
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
