# Changelog

## v0.12.0 — 2026-06-22

### Added
- **In-thread MCP permission cards** — MCP tool calls in regular chat now prompt with an inline permission card (remember-per-thread, silenced for triggers), replacing the old consent-prompt flow.
- **Official Lucidos marketplace suggestion** — the App Store suggests the official marketplace when none is registered.
- **Gateway reload control** — reload the gateway from the workspace picker with a new-build badge and status/reload endpoints, plus a refresh/restart control on the current workspace row.
- **Whole-file end-state diff toggle** — toggle any diff to view the full merged end-state of a file, not just the change hunks.
- **Plugin-ownership-aware app delete** — deleting a plugin-owned app is blocked and redirects you to uninstall the plugin.
- **"Include deleted" thread filter** and a dedicated **Running** view in the thread drawer.

### Changed
- Thread drawer consolidated — view selector + channel filter merged into one **Filter** dropdown (Lucidos / Coding Agent / Triggers icons), attention-only badging, a unified running spinner, and section-header icons.
- Compose flow refined — auto-open the coding-agent dropdown after picking a coding destination, a round Send/Stop button, and a mobile split button for change actions.
- Engine-restart UX softened — no more full-UI deactivation; a light, dismissible toast driven by a reliable build-id check instead of the fragile service-worker event.
- Dual Submit/Cancel control for a pending question or permission; the Lucidos brand mark now represents the Lucidos Engine actor.

### Fixed
- Large hardening wave — 110+ fixes across iOS-PWA boot splash and push deep-linking, the gateway cold-start picker redirect (deep-link query+hash preserved, per-workspace localStorage via the Storage prototype), drawer / compose / mobile layout, MCP permission-denial accounting, trigger-CRUD actor preservation, zombie-thread recovery, Apply-All batch-toast rehydration, light-theme token drift, and dropdown / filter interactions.

### Removed
- App-UI time-travel (serving, versions/restore endpoints, and the frontend), the cc-edit-preread Read-before-Edit guard, and the prompt cancel confirmation.

## v0.11.0 — 2026-06-18

### Added
- **Multi-workspace gateway** — standalone `lucidos-gateway` crate plus `lucidos-engine --gateway` mode, `/<slug>/` + `/~/` routing, engine-served frontend with base-path awareness, one shared dev gateway with per-workspace auto-start, and a brand-blue card-less workspace picker with animated mark, in-app switcher, and management UI (ADR 0013/0014).
- **Self-contained macOS desktop app** — single `.app` bundling PostgreSQL 18 + engine + JS SDK, `build-dmg.sh` packaging, signed + notarized DMG + updater artifacts, auto-update from GitHub Releases, always-on LaunchAgent service + Tailscale mobile access, `File -> New Window` (Cmd+N), one-click `curl | sh` installer.
- **DB-backed environment variables** — store, API, injection, and `request_credential` LLM tool with custom env-var-name pre-fill; Settings subview, nav router, and live SSE updates.
- **Restore-from-backup in the workspace picker** + `restore-archive` engine subcommand (old Settings restore surface removed).
- **App Store folded into Apps** — Installed/Store tabs, marketplaces, and auto-update for marketplace installs.
- **Per-workspace localStorage namespacing** behind the gateway.
- **Brand identity** — Lucidos mark as a brand component, regenerated native Tauri icon set, boot splash + workspace-starting splash, dark-blue (logo-hue) theme, logo-reveal animation, favicon on the boot splash.
- **OpenRouter (GLM 5.2) + local OpenAI-compatible LLM backends.**
- **Pane/keyboard focus system** — focus panel on header click, prompt-focus drives thread pane, per-pane Tab trap, focused-pane Back/Forward nav.
- **Nav-history dropdowns** — history list on long-press / right-click of the Back/Forward chevrons, with thread-type and content-category icons.

### Changed
- Codex mid-turn follow-ups interrupt-and-redirect the live turn.
- Tighter coding-agent commit cadence + post-commit diff display.
- Thread drawer toggle is now a plain show/hide (not a focus stage); Current threads sorted by creation time.
- Bundled PostgreSQL 17 -> 18 with automatic data migration; shared Postgres `max_connections` raised to 500.
- Brand-blue header bar with light foreground inversion; dark theme retinted to very dark blue.
- Markdown parsing cached so a re-render doesn't re-parse the whole thread.

### Fixed
- macOS notarization — sign loose bundled PostgreSQL Mach-O binaries inside-out so the notary accepts the DMG.
- PWA shows the gateway boot/stopped (503) splash instead of the stale cached shell.
- Queued-message trash icon rendered inline within the status label.
- Stored env vars applied to the engine process at startup; credential custom env-name is additive.
- Large hardening wave — 130+ fixes across the gateway, desktop packaging, auto-update, notifications, drawer/mobile layout, focus handling, changes/apply, and engine recovery.
## v0.10.0 — 2026-06-15

### Added
- **Codex as a second coding-agent backend** — per-thread backend selection (Claude Code or Codex), app-server driver with permission cards, streaming and graceful interrupt; Codex taught the `lucidos` CLI + `ask_user_question` MCP tool (ADR 0005).
- **Thread Queue** — system-wide admission control for all thread work; background spawns and user-initiated work share one capacity pool (ADR 0008), with a Thread Queue panel (Run now / Drop / edit policy) and policy tools.
- **App coding-agent threads** — folder-scoped CC/Codex threads scoped to `data/apps/<id>` with a compose scope picker, WIP preview, app branch chip, and app-building knowhow served via the `lucidos knowhow` CLI.
- **Command Safety** — interactive permission lane for chat commands, an LLM judge for the ambiguous middle, a static catastrophic-command block, checkpoint + undo for the reversible lane, and trigger side-effect grants (ADR 0002); grouped under Settings -> Permissions.
- **Model providers** — OpenAI direct provider in Settings -> Providers, background tasks routed to OpenAI, a DB-backed model registry, direct Anthropic provider, Claude Opus 4.8 (now default, incl. 1M), and Fable 5.
- **New agent tools** — `run_python_background` for long-running scientific-Python work, and `count_events` plus byte-budgeted `query_events`.
- **Inline file editing** in the file preview.
- **Backup** — persistent backup status on Settings -> Backup, `data/.backupignore` support, auto-generated key on scheduled backups, and a "View backups folder" link.
- **Notifications** — native macOS push for the Tauri desktop app, Declarative Web Push for iOS, and structured `Tap` deep-link routing from the inbox.
- **New-workspace welcome** with clickable starter suggestions; a single compose **destination picker** replacing the mode toggle + scope/agent chain.
- Lucidos **theme inheritance is now the default** for new apps.
- Wake-question (single-option ask) for genuinely unbounded waits.

### Changed
- Thread drawer reworked: needs-attention sorting by review tier, sort by last user action, context chips, status-dot tooltips, merged Active + Review into one **Current** section.
- Every overlay migrated onto a unified `<Overlay>` component owning the dismiss contract; UI behind an open overlay is now inert.
- Mobile navigation is swipe-only (drawer/content toggle icons dropped); raised the per-turn tool cap to 500 with a banner when reached.
- Knowhow no longer stamped into app `manifest.json` / SDK `App` type.

### Fixed
- Large stability sweep: iOS-PWA blank/black thread recovery, deep-link scroll, auto-scroll re-pin, restart-overlay layering, notification badge/unread sync, change apply/discard idempotency and thread-state gating, worktree cleanup of stranded/orphaned worktrees, and many e2e flakes (WebKit reaper, Playwright 1.60).
- Project-wide clippy/harden passes and large module splits (files-under-1k refactor).

### Removed
- Legacy `ModalOverlay` component (everything migrated to `<Overlay>`), the CC allowed-tools settings section, separator dividers app-wide, the compose mode toggle and "Discard draft" button.
## v0.9.9 — 2026-06-05

### Added
- Inline text-file editing in the file preview pane.
- "Thread" button on changes-panel rows to jump to the originating thread.
- `--built` frontend dev mode (now the default; `--hmr` opts back into the live Vite dev server) to kill the iOS PWA cold-load black screen.
- Clickable links rendered inside AskUserQuestion question text.
- Mobile: "Keep header visible" now defaults to on.
- Active service-worker BUILD_ID surfaced in the control panel.

### Changed
- PWA caches the navigation shell so a notification-tap reload boots from disk; faster iOS notification-tap reload overall.
- Restart-overlay z-ordering: only toasts sit above the overlay; fullscreen app, landscape lock, drawer threads, and tooltips drop below it during restart.

### Fixed
- CC: Cancel now acts like Esc (interrupt + resume) instead of kill + respawn.
- CC: external-repo agents stay on the worktree branch so the Diff tracks their PR.
- CC: only set `RUSTC_WRAPPER=sccache` when sccache is on PATH; set it to `""` (not unset) when absent.
- Diff button gated on the same algorithm the viewer uses (hidden when empty); falls back to `origin/<default>` when the local default branch has diverged.
- Steps: redact ToolCalled description from masked args; friendlier progress labels for generic-fallback tools.
- Backup: "Show backup key" is read-only; key generation is explicit and never overwrites.
- Notifications: route warm Chrome push taps via postMessage instead of fragment navigate.
- File preview: freeze the editor fetch URL at mount so revision bumps don't tear out the textarea.
- Scroll: chevron re-engages the tail; snap before resize so big chunks keep following.
- dev script: mode-aware `show_banner`, kill stale `--built` build-watch, guard `${BUILT:-}`.
## v0.9.8 — 2026-06-03

### Added
- **Per-workspace environment overrides.** The engine now loads `<workspace>/data/.env` at startup via `dotenvy::from_path_override` (override semantics) and injects the result into every subprocess it spawns (`run_bash`, `run_python`, Claude Code, triggers). The motivating case is a per-workspace GitHub identity — point `GH_CONFIG_DIR` / `GIT_SSH_COMMAND` at the right account so `gh` / `git push` from agent subprocesses use the correct credentials. The file is gitignored (`data/.env` added to the workspace gitignore).

### Changed
- Documented the per-workspace `.env` behavior in `README.md` and corrected the "git-tracked under `data/`" rule in `docs/taxonomy.md` to list the real gitignored exceptions (`postgres/`, `blobs/`, `.env`).
- Added agent-facing knowhow for `data/.env` setup in `system-knowhow/best-practices.md` (override semantics, subprocess inheritance, the GitHub-account recipe, and the ⚠️ restart-required-after-edit callout), with `system-knowhow/workspace-audit.md` kept aligned.
## v0.9.7 — 2026-06-03

Three weeks of work since v0.9.6 (1111 non-merge commits). Headline themes: a full rebuild of notification/push delivery around a live presence protocol, app coding-agent threads, collapsible thread families + groups in the drawer, a customizable keyboard-shortcut system, structured notification taps, and several new chat-agent tools.

### Added
- **Live presence-based push delivery.** New `PresenceCheck` SSE protocol: engine pings devices and waits for a `POST /api/presence-pong` before deciding whether to push, replacing stale-heartbeat guesses. Fan-out rewritten around a pure `decide_push_allowed`; per-device dismiss-on-read; daily auto-disable of push on stale devices.
- **Declarative Web Push for iOS** plus engine-scheduled wake-push and a periodic service-worker liveness probe to work around the macOS-Chrome SW wedge.
- **Structured notification `Tap`** — `{kind, to?}` discriminated union replacing the old `'modal'|'open_app'|'open_thread'|'none'` strings, with `tap=none` passive auto-read notifications and event-anchored deep-links.
- **App coding-agent threads** — spawn a Claude Code session scoped to a single `data/apps/<id>/` folder (sparse-checkout worktree, ff-merge on apply, no engine restart). Compose-view scope picker, WIP preview, app-branch chip, and a two-layer guard so a CC thread can't kill its host engine.
- **Collapsible thread families in the drawer** — child threads render under their parent with a toggle row, saved-section attention badges, and `blocking_descendant_count` plumbed through the projection.
- **Trigger groups** — `TriggerGroup` entity + events, HTTP API, LLM tools, collapsible group sections in the triggers panel, and a group picker in the trigger detail modal.
- **Customizable keyboard shortcuts** — registry-driven keybindings with override persistence (synced as a workspace preference), an interactive recorder, a cheat-sheet searchable by combo, and a non-destructive Escape/close-cascade dispatcher (Cmd/Ctrl+W, Cmd/Ctrl+Shift+W).
- **New chat-agent tools**: `todo_write` (live todo list + `TodoListWritten` event), `run_python_background` (long-running scientific-python), `count_events` (byte-budgeted event queries), `list_changes`/`apply_change`, and `lucidos changes list/apply <id>` + `--folder` app targeting on `spawn-thread`.
- **Models**: Claude Opus 4.8 (now default, plus 1M-context variant in the CC picker), Gemini 3.5 Flash, Haiku 4.5.
- **OAuth provider registry** — credential modal pre-fills auth/token/userinfo URLs from a built-in registry (`well_known_provider` renamed to `known_provider`) and auto-expands a custom-URL section for unknown providers; registry extended with Spotify. New `lucidos.oauth.getAccessToken(provider)` SDK method + `GET /api/v1/oauth/{provider}/access-token` for in-browser SDKs (e.g. Spotify Web Playback).
- **Full credential editing** with email settings and masked secret reveal.
- **`.backupignore` support**, persistent backup status on Settings → Backup, and auto-generated backup key with a store-this-key prompt.
- **Per-thread loaded-knowhow tracking** — `[LOADED KNOWHOW]` injected each turn, recovered from events on restart, body stripped from history.
- Actor-stamped events across 13 mutating endpoints; `ImageDescribed`, `EngineSupervisorRespawned`, `PreferencesChanged`, `EmailSent`, `ProxyModulesReloaded` events.
- Cross-workspace `run_claude` (`workspace` param) and a watchdog that auto-resumes stuck/hung CC sessions.
- Configurable, collision-aware Vite port selection pinned to `lucidos.toml`.

### Changed
- Raised the per-turn tool-call cap to 500 with a banner when reached.
- Chat agent nudged to use `ask_user_question` for choice-shaped follow-ups; forbidden from parallel-calling it; AskUserQuestion card shows single- vs multi-select mode and renders option descriptions as markdown; wake-question single-option variant for unbounded waits.
- Cascade-archive the whole thread family in a single transaction.
- SPA overlay surfaces lazy-loaded (~40% smaller main bundle); context modal/sections lazy-fetched after snapshot strip.
- Diff/Apply button driven by a single `ccBranchHasDiff` signal instead of a three-way union; `branch_has_diff` seeded on session bootstrap and refreshed by the startup recovery sweep.
- Editable subject/body in the email confirm dialog; cross-workspace Origin popover shows thread name + link.

### Fixed
- Hundreds of fixes across chat, drawer, notifications, backup, archive, service worker, coding-agent, mobile/PWA, and e2e. Highlights: thread-title Escape cancels without saving; backup auto-key on scheduled runs; iOS edge-navigation gesture suppression; macOS-Chrome SW notification wedge mitigations; active-children count reconciliation; CC watchdog recovery from internet outages.

### Removed
- Page-side notification wedge-recovery Layers 3+4 (superseded by the engine PresenceCheck path).
- Temporary iOS push-tap diagnostic breadcrumbs; broken chat model-fit guard (reverted); dead `SHORTCUT_IDS` export and assorted dead code; sync `run_python` write-guard.

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
