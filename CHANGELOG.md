# Changelog

## v0.15.0 — 2026-07-03

### Added
- **New-version / Switch flow** — Apply now auto-builds the engine in the background and splits *build* from *switch*: a unified "new version available" surface, an `ENGINE_BUILD_ID` + `/engine/version-status` endpoint, cause-gated resume that preserves pending questions, and switch-as-respawn with boundary events deferred to teardown. The Apply button reads **Apply\*** when a restart is required; the brand badge shows a "!"/spinning build icon instead of a count; version toasts defer on dismiss with a persistent update badge; a hint toast explains when a frontend-only Apply is deferred to Switch. The engine also surfaces when its source is *behind* the running binary and self-heals a failed background rebuild; the pending-Rebuild toast only shows when no switchable binary already exists.
- **Keyboard navigation of thread content** — arrow-key scrolling and turn-by-turn traversal (Cmd+Up/Down), Enter to collapse/expand the navigated turn, a unified deep-link "navigate to element" + chevron-scroll model with perceptible deceleration, and a persistent deep-link focus marker ("focus stick").
- **Focused-pane cue** — the focused pane's header segment gets a subtle lighter-blue wash (extended over the divider seams) so the active pane reads at a glance, mirrored on the mobile active-pane dot. (The earlier header focus *pill* was tried and dropped in favor of the wash.)
- **Animated compose height** — the prompt textarea animates its height (and position) on send and on draft↔draft / draft↔blank compose-view switches, with in-flight/rapid-switch frames cancelled cleanly.
- **Thread drawer overhaul** — the compose-draft tooltip becomes a ⋯ menu (Delete + Info), fully keyboard-drivable; a keyboard shortcut expands/collapses the focused thread's sub-threads; single-focus (aria-activedescendant) model.
- **Remote access & Linux install** — `install.sh` downloads a prebuilt cross-platform headless tarball by default and registers the gateway as a user service; opt-in TLS (`--tls-cert`/`--tls-key`) and network bind (`--bind` via `network.toml`); a user-facing `scripts/run.sh` entry point; `build-headless.sh` + CI matrix for the Linux tarball; post-extract execution smoke + runtime-dep preflights.
- **Self-skeletonizing loading system** — skeleton primitives with a fill helper, tree-shaped skeletons for files/repos, self-skeletonized list rows, drawer search, and triggers; retires the generic `ListSkeleton`.
- **Per-thread model memory** — each thread remembers its last model + reasoning effort; coding-agent threads are pinned to their first account.
- **Native dock badge & notification actions** — a nudgeable dock-badge loop driven by notification SSE, an on-demand unread-total endpoint, and `[Open]`/`[OK]` action buttons on in-app notification toasts (web links in toasts are now clickable).
- **Pane-anchored toasts** — each toast is pinned to and centered over the pane it appeared on.
- **`--font-size-*` type scale** — host + app-iframe type-scale tokens, with all font-size literals migrated onto them.
- **Per-draft compose state** — compose-view dropdown selections and attachments persist per draft in the DB.
- **macOS menu-bar mode** — the desktop client goes menu-bar-only when its windows close.
- **Durable native device id** — the packaged desktop app persists its device id natively, so reinstalls keep a single device instead of spawning duplicates.
- **Trigger last-run status** — triggers surface last-run OK/failed status and a build-on-top pointer.
- **Codex key auto-detect** — the engine auto-detects an OpenAI key from the Codex CLI auth file; the chat agent is nudged to emit clickable `[Name](app:<id>)` app links.
- **Graceful memory degradation** — the engine boots with memory degraded (instead of failing) when the embedding model can't download, with an actionable error.

### Changed
- **Apply/Discard** — the Diff button is permanently pulled out of the split button; frontend-only Applies propagate to peer dev workspaces and re-snapshot the served frontend without a respawn.
- **App-name links** — stopped auto-linking bare app-name mentions in chat/notifications in favor of the explicit `[Name](app:<id>)` form.
- **Plugin setup threads** — setup instructions moved to knowhow with a short seed message.
- **Navigation focus marker** unified across chat, settings, and plugins (drops the entrance flash, fades on any user action).
- **Lockfile determinism** — builds use `npm ci` and `cargo --locked`; the `lucidos` CLI is bundled as a runtime resource in the packaged app.
- **Vertex** — `eu` multi-region added as a prefilled region option.
- **Spacing & type consistency** — content-pane padding unified onto a `--space-*` scale (Files tree + cards are deliberate carve-outs); prompt/confirm modal font sizes normalized with a codified no-magic-font-size rule.

### Fixed
- **Security** — closes a CRITICAL CVE via wasmtime 25→36; blocks cross-origin browser-proxy requests; hardens `http_request` credential redirects; `git2` 0.20.4 + safe Rust security bumps; scrubs credentials from the packaged env.
- **Vertex Gemini 3 reasoning** — uses `thinkingLevel` + `includeThoughts` so reasoning stops leaking into the answer, and never sends `thinkingBudget` to Gemini 3 (clamps `thinkingLevel` per model).
- **Coding-agent reliability** — stops a Fable false stale-resume from spawning duplicate CC processes / deleting live worktrees; an external watchdog kills a wedged coding-agent subprocess on recovery; preserves genuine CC API-drop failures instead of fabricating "Unknown error"; recovery reuses a worktree only when it's on the branch being recovered; a startup lease serializes restart recovery; `CLAUDE_CONFIG_DIR` is pinned per session so a mid-flight provider toggle can't strand a resume; reads CC `thinking_delta` from the correct field.
- **Packaged runtime** — packaged PATH floor + agent-binary detection (psql on the coding-agent PATH); resolves user-installed tools under the service-manager minimal PATH; boot preflights (git, PG client, embedding model, required resources) with actionable warnings; Docker entrypoint aligned to the PG18 binary path.
- **Changes/Archive** — reconciles an orphaned pending change that blocked Archive; gates apply-time reconcile on Applied and advances Apply-All on discard; the Apply-All merge-conflict toast no longer dangles.
- **Streaming resilience** — bounds the streaming send-header phase so a stalled LLM connection can't hang a turn; idempotent thread pin/unpin so a double-submit can't 409.
- **iOS/mobile** — repaint scroll-nudge no longer cancels momentum scroll; reliable scroll-to-top; boot-splash covers the iOS safe-area strip; mobile header titles centered without overlapping leading icons.
- **Chat** — repaired leaked inline tool-call XML in the agentic loop; the title model no longer executes instruction-style prompts; the agent is told its reasoning isn't shown to the user and is forbidden from claiming a repeated action without a fresh tool call; the coding agent must not ask post-work confirmations that block Apply (scoped to Apply-based prompts).
- **Cancel** — releases a stuck "Canceling" state when a running-turn cancel is superseded into waiting-for-answer, and clears the awaiting-bit on cancel rollback.
- **Build lock** — fails open (rather than reporting SkippedLocked) when the checkout is unresolvable; scoped to engine-triggered builds.
- **Drawer / notifications** — restored the normal drawer scrollbar and dropped the right-inset selection gap; channel/error tags keep their dark-mode hairline and red outline; notifications panel toolbar padding aligned with the plugins panel.
- **Toast / focus** — collapsed-pane toasts recenter over the surviving pane; toast focus/tab handling hardened for touch and overlays.

### Removed
- Retired the generic `ListSkeleton` component (replaced by the self-skeletonizing system) and a stale `crates/lucidos-app/package-lock.json`.
## v0.14.0 — 2026-06-29

### Added
- **Network access UI** — configure the engine/gateway network bind from Settings and the workspace picker; durable scope-split bind (gateway machine-global, engine per-workspace), with click-to-fill of the detected Tailscale IP.
- **Plugin Modified badge** — the Plugins list now shows a per-plugin *Modified* state derived from the install commit, and warns before an update would overwrite local edits.
- **App-icon unread badges** — native dock-icon badge with the aggregate unread total, per-workspace PWA app-icon badges, and gateway-aggregated per-workspace counts.
- **Cross-device native notification dismiss** — dismissing a notification on one device removes the delivered native banner(s) on your other desktop devices.
- **Documentation site** — mkdocs-material docs site with anti-drift transclusion and deploy-on-release.
- **Public-repo RC gate (CI)** — clean-machine source install + signed-DMG verification on fresh macOS/Ubuntu runners before publish.

### Changed
- **Vertex AI region** moved into the Settings → Providers section.
- **OAuth account resolution** — provider tokens now resolve to the newest connected account; the Accounts UI shows a created date per account.
- Sleeker Network access modal; skeletons fill the full content height; notification-detail title sized to match the list row.

### Fixed
- **Security hardening** — engine API now defaults to a loopback bind (opt in to all-interfaces via `LUCIDOS_BIND_ALL`); gateway control plane is authorized against app iframes; WASM signer execution budget + credential-leak scrubbing; scoped credential-URL matching.
- **Networking** — retain loopback when binding a specific address; bounded gateway boot-splash escape; route us/eu Vertex multi-region locations to the `rep.googleapis.com` host.
- **Workspace storage** — workspace-scope *all* browser storage (theme, device-id) with an idempotent namespacing override and a regression guard.
- **Frontend recovery** — recover gracefully from navigating into an unreachable workspace; gate the picker skeleton behind a 300ms delay to stop the fast-load flash.
- **iOS PWA** — fix the blank thread body (compositor paint loss) via a scroll-nudge + forced layout flush.
- **Notifications** — reliable native desktop banners with durable deeplink; the agent is now aware it's running in the Tauri desktop app.
- **Cross-platform install** — OS-aware sleep/clamshell prevention so Linux source installs run clean.
- Muted-gold light-mode warning toast.
## v0.13.1 — 2026-06-28

### Fixed
- **Toast in light mode** — toasts read gray on a light background; now use a white fill so they render cleanly.
## v0.13.0 — 2026-06-28

### Added
- **Capability-parity manifest + grouped agent tools** — a single capability manifest is the source of truth for the agent's tool surface, with Rust→TS codegen keeping the LLM tools, the `lucidos` CLI, and the JS SDK in sync. Many narrow tools are consolidated into grouped tools — `triggers`, `trigger_groups`, `preferences`, `events`, `changes`, `mcp`, `plugins`, `threads`, `thread_queue`, `memory`, `manage_models`, `manage_repositories`, `env_vars`, `notifications`, and an `apps` domain — each gaining matching CLI subcommands and SDK methods.
- **Agent-configurable settings** — the agent reads and changes user preferences via `get_preferences` / `set_preference` (theme, language, timezone, push, welcome message, chat model, reasoning effort, UI scale, font, …), validated against one catalog and routed through a single write chokepoint so per-device scope and live-apply are handled automatically. The narrow `set_language` / `set_timezone` / `enable_push_notifications` tools fold into `set_preference`. A `manage_models` tool adds/enables/disables/removes chat models in the picker. Language + timezone also get human controls under **Settings → System → Locale** (with IANA timezone validation). The command guard stays human-only.
- **Dedicated Plugins panel** — browse, install, and update plugins with an "Installed only" filter (default shows all), controlled-vocabulary categories, plugin updates appliable from the list, and provenance-tracked auto-registration of plugin triggers. Installed/uninstalled files are git-committed into the workspace repo.
- **Coding-agent reasoning in the timeline** — streamed model reasoning is captured as `CodingAgentThoughtStreamed` and rendered as a live "Thinking" step with full persisted text.
- **Loading-state overhaul** — a `ListSkeleton` primitive plus skeleton-by-default loaders, a 300/500 minimum-visible standard (retiring `DelayedSpinner`), and skeleton + fade-in + prefetch for the thread-open transition; loaders crossfade out via `LoadingFade`.
- **Settings → System → Debugging** panel with a default-off perf-instrumentation toggle; thread-open render/paint timing is instrumented and flushed via a batched `/internal/client-logs` telemetry endpoint.
- **Thread overflow (⋯) menu** with a separate pin icon — Archive and thread Info (moved out of the hover tooltip) live in the menu; Archive first, Info last.
- **SDK `ui.toast` + `ui.prompt`** host-bridged components for apps.
- **Fira Code font option** with programming ligatures.
- **`get_backup_status` tool + backups knowhow** — timezone-aware backup scheduling, agent-readable/writable backup settings, and persisted run history.
- **Targetable memory entries** — entry IDs surface in the `[Long-term Memory]` block and the Memory settings view (copyable), plus `correct_memory_by_id` to delete/replace one memory by id.

### Changed
- **Welcome screen redesign** — compact "Hi, there!" hero with a chevron suggestion carousel (one idea at a time), conversational clickable starter suggestions that prefill the prompt, and a top-right dismiss pill.
- **Toast redesign** — elevated surface with a thin per-type category-colored border on a plain theme background, clean amber warning in light mode, full-width centering, per-theme tint strength, and the "New version available" toast replaced by the refreshing spinner.
- Files panel: "Drop or click to import" pinned to the top-right of the source-switcher row; Expand/Collapse-all removed; empty repo Files toolbar no longer rendered.
- Settings: Animation speed moved from Appearance to System → Debugging; Environment Variables indexed in search; redundant subpanel titles dropped.
- Notifications: detail chevron navigation walks the whole inbox (not one page); detail panel uses larger body text and primary color; chevron layout refined for two-line titles and the iOS-PWA back-swipe gutter.
- Drawer/Archive: Archive pile is a single global created-at window (gap-free, chronological); long context-name chips wrap instead of truncating; tree-style ←/→ keyboard navigation.
- Mobile pane state persists in localStorage so a PWA close reopens where you left off.
- "App Store" wording retired in favor of the plugin **Store**; English recommended in the language setup prompt and under the Language setting.
- E2E suite runs against a release build by default.

### Fixed
- **Security hardening** — guard path traversal in the intent loader and browser screenshot path; floor char boundaries on OpenAI streaming token slices; scrub private/internal data from public-shipping sources, docs, system-knowhow, and test fixtures, backed by a fail-closed private-data release guard.
- **Drafts** — a locally-edited draft is no longer blanked by a `MessageReceived` echo or a bulk SSE resync (drafts:65 `value=''`).
- **Coding agent** — worktree `node_modules` provisioned from the hoisted root (npm workspaces); engine restart during a resumed session no longer read as a user rejection; idle-termination and question-answer-resume races closed; run-loop flags reset on resume.
- **Gateway** never respawns an alive engine, only a dead one.
- **Apply** — an already-merged branch is marked applied (instead of stranded as failed) and pushes main on a no-op.
- Chat: no spurious "No changes" flash in Changes-mode; no open-jump on change-applied threads; pending messages survive a transient safety-refetch failure; iOS open-path repaint hardened.
- macOS app menu derived from the system default so arrow keys move the cursor; About item labeled "About Lucidos".
- Numerous UI fixes: dark-mode green confirm buttons, boot-splash label/sizing aligned to the gateway, header divider/tooltip behavior, and welcome carousel chevron/height stability.

### Removed
- Retired flat/narrow agent tools now superseded by grouped tools; dead components (`DelayedSpinner`, `StoreTab`, `ExportThreadButton`, `CopyThreadRefButton`).
## v0.12.5 — 2026-06-25

### Added
- **Welcome screen redesign** — compact "Hi, there!" hero with a chevron suggestion carousel (one idea at a time), conversational de-quoted starter ideas, and a top-right dismiss pill. New starter suggestions: app store, mobile-access setup, daily scraper, weekday email summary.
- **Targetable memory entries** — memory entry IDs now surface in the `[Long-term Memory]` block and the Memory settings view (copyable), plus a new `correct_memory_by_id` tool to delete/replace one specific memory by id.
- **Thread Queue moved into Settings → System.**
- Files import hint relabeled to "Drop or click to import" in a dashed drop box.

### Changed
- Gateway boot-splash phase renamed to "Downloading memory model".
- Settings: Environment Variables now indexed in search; redundant subpanel titles dropped.
- Notifications: a trayed/unfocused Tauri window now counts as not-in-use, so push is delivered when the window isn't actually visible (device-presence re-sync deduped on native focus change).

### Fixed
- macOS app menu derived from the system default so arrow keys move the cursor; About item labeled "About Lucidos".
- Tauri: nav-history popover renders above the internal browser; app menu placed below the header strip; window drag and thread-toggle no longer steal the focused pane.
- Settings: API URL renders at normal row size (not page-base); system subpanel tabs render on the Environment Variables view.
- Threads: thread-link hover shows the real destination; cross-workspace thread links route through the gateway; stopped peers aren't lazy-started just to read a title.
- Update badge + toast unified on the build-id check; switcher reload icon badged.
- Welcome surface shows until dismissed and no longer clips the empty compose box.
## v0.12.4 — 2026-06-25

**Fixed**
- **Desktop window state persists across launches** — Tauri window size, position, and screen are restored on relaunch, and the window-state save is marshalled onto the main thread.
- **Welcome message** now shows until dismissed and no longer clips the empty compose box; added top padding and dropped the tagline.
- **App menu** "About" item is labelled **"About Lucidos"** instead of "Lucidos".
## v0.12.3 — 2026-06-25

**Added**
- **"See all statuses" shortcut** — empty status-filter views in the thread drawer now offer a one-click way to clear the filter and see every thread.

**Changed**
- **macOS menu-bar tray icon** rendered as a proper monochrome template glyph that fills the canvas (correct light/dark menu-bar appearance, no padding frame).
- **Window dragging** works from the whole header strip, with maximize-on-strip and a focused-pane accent line under the header.
- **Focused-pane marker** tuned to a full-width underline, muted in dark mode; navigating now activates the focused pane group.
- **Quit menu item renamed**, and the app confirms before stopping the background service.
- Chat agent now anchors on the currently open app/file for UI/copy requests.
- README tagline sentence-cased ("If you can describe it, it exists").

**Fixed**
- Packaged builds no longer register the workspace as a "Lucidos source" repository.
- Repo HTML shows as source (not a live render) in the file/diff preview.

## v0.12.2 — 2026-06-25

**Added**
- **Menu-bar tray model (macOS)** — the always-on service now survives closing the client window; the engine keeps running in the menu bar.
- **In-app "Uninstall Lucidos" command (macOS)** — clears WKWebView web storage and hides client windows on confirm, with a keep-vs-delete data choice, so a reinstall is clean.
- **First run shows the workspace picker** — no more silently auto-created default workspace; offers personal/work name suggestions.
- **`view_image` chat tool** — reprocess images posted earlier in a thread back into the agent's vision.
- **macOS title-bar tinting** — the native title-bar strip is reclaimed as a blue drag-band matching the app header.
- **Gateway boot-phase progress** — the workspace boot splash now renders engine-reported boot phases instead of a blank wait.
- **Keyboard pane navigation** — focus the Conversation drawer (Cmd+Shift+1) and maximize the focused pane group (Cmd+Shift+Enter).
- **`.gs` (Google Apps Script) files** highlighted as JavaScript in the file preview.
- **Unattended trigger-spawned coding agents** — trigger-spawned sessions inherit the side-effect grant and auto-resolve permission prompts.
- **Two-phase release pipeline** — `release.sh --verify-build` / `--publish-verified` and an `--attach-staged` path that builds once and verifies before publishing.

**Changed**
- **Security: permissive CORS disabled by default**, and the **gateway default bind address secured** (no longer binds broadly out of the box).
- **Resolved npm-audit vulnerabilities** — vite 6.4.2→6.4.3, @babel/core 7.29.0→7.29.7.
- Packaged update now restarts the whole service and surfaces inside the workspace; the dev-only gateway reload is hidden on packaged builds.
- JSON API responses are gzip/brotli-compressed via tower-http.
- Deterministic root-commit repository identity, with orphaned-thread backfill.
- Per-parent child-thread fan-out cap raised 3 to 10.
- Single-file changes open directly into their diff; added files render as the whole file.
- User-facing thread "Saved" renamed to **"Pinned"**.
- Cron fires coalesce so a trigger holds at most one queue entry (idempotent recovery).
- "Lucidos source" coding target hidden on packaged builds; capture-context debug toggle defaults off.
- Unified focus ring across buttons/dropdowns via a `--focus-ring` token (collapsed to a single soft band).
- Perf: windowed thread render, faster exchange sort + incremental pending-message fold, memoized drawer categorization — fixes dev-workspace input lag.

**Fixed**
- Engine no longer culls alive-but-busy engines (gateway respawn-storm fix).
- Graceful coding-agent process-group teardown so Playwright reaps its browsers.
- Concurrent worktree spawns no longer collide on `.git/config.lock`; backoff sleep skipped on the final retry.
- Attached images stay visible to the agent for the whole turn; image message protected from context pass 2.
- Avoid a UTF-8 panic when truncating memory context.
- Coding agent treats relative `..` targets as out-of-workspace; unwraps `shell -c` before classifying Codex commands.
- Globally disable browser autofill on host-app text inputs.
- Render binary images in the repo file viewer.
- Archive drawer ordered by created_at; inbox threads excluded from the archive pagination cursor.
- Hard-exit after uninstall so the window-state plugin can't re-create the deleted data dir.
- Various: picker boot-splash text, "Manage workspaces" link on a direct engine port, app-thread change-row Diff, legacy workspace switcher list, per-pane Tab trap, thread-row "Waiting" tooltip.
## v0.12.1 — 2026-06-23

**Added**
- **Hot-swap LLM provider on credential change** — adding or changing a provider key takes effect on the next chat, no restart required.
- **Provider-aware first-run onboarding** — a fresh workspace with no LLM provider configured guides you to Settings → Models → Providers instead of silently serving mock output; the engine reports `llm_configured` via `/health`.
- **Vertex AI in packaged builds** — Application Default Credentials (ADC) auto-read; the model list is filtered to only configured providers.
- **Workspace-picker boot recovery** — a wedged boot splash now reveals an escape link to the picker; health-gated auto-open and first-run workspace naming.
- **Notification detail in the content panel** instead of a modal.
- **Action-toast keyboard support** — focus, Tab cycling, and a visible focus ring.

**Changed**
- Prompt answer/follow-up SplitButton unified into one frosted, same-width frame.
- History-navigation arrow defaults swapped: Forward = Up, Back = Down.
- Setup copy corrected to Settings → Models → Providers; stale "restart" wording dropped.

**Fixed**
- Bundled engine is now self-contained — OpenSSL is statically vendored, so the packaged build no longer depends on Homebrew OpenSSL (the crash-loop that blocked packaged startup).
- Self-healing embedded Postgres lifecycle — stops on shutdown, adopts a healthy running instance, version-guarded.
- Never serve mock LLM output on a no-provider boot.
- Toast button focus ring no longer shows on mobile/touch.
- Laggy repeat-tap can no longer cancel a just-sent turn.
- Question/permission divider no longer mislabels system aborts as "Canceled".
- Diff view resets to hunks on each new diff.
- Stranded "Apply Now" toast is cleared on resume.
- Workspace selector renders above toasts.
- In-body notification app links open via openAppById, so disk-backed apps no longer falsely report as missing.
- Keyboard-shortcuts label font size normalized.
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
- **`HEAD` is current** — re-applied lost contributor fixes (Ctrl+Shift+O on Mac, history-collapsed pagination guard) with regression tests.
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
