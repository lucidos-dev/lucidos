# Changelog

## v0.21.0 — 2026-08-04

### Added

- Question cards and permission cards are keyboard drivable. One choice holds focus the moment the card appears, so Enter answers it; the arrow keys step between choices, and the focus ring is visible wherever a choice is seeded.
- A reminder bar appears when backups are off, with a link straight to the backup setting. Dismiss it and it stays dismissed.

### Changed

- Live question cards name the two escapes on the card itself: type any reply in the prompt, or cancel the question. The dead-end "Other" option is gone from every question the model can ask.
- The workspace picker setting says what it decides, that the workspace keeps working while no window is open. New workspaces default to it, and existing workspaces are lifted to that default once.
- Mobile Access reports the engine host's tailnet state separately from the device you are reading on, and stops offering an install to a browser that will refuse it.
- Lucidos tells you when its database is unreachable. Health reports the database separately from the process, the boot splash stops waiting instead of hanging, one toast names the outage, and the launcher offers to start Docker when Docker is what is down.
- Toasts are anchored to the bottom edge of the header, so they clear whatever the header is currently showing.
- Each published release on the public mirror is now a child of the previous one. Clones of the repository keep working across releases instead of breaking on every publish.

### Fixed

- The prompt no longer jumps a line as its placeholder changes; the placeholder is measured, and the measurement stands down while the card animation eases.
- The hint on a question card reads in the prompt, where the answer is typed, instead of under the answers.
- A link in the chat can no longer reload the whole workspace.
- Prose detail rows in Settings flow inline instead of being laid out as fields.
- A generated image is described by what it shows, not by its position in the thread.
- On mobile, archiving from the thread drawer stays in the drawer instead of swiping to the thread pane.
- The scroll pin at the bottom of a thread cannot spin when its suppression timer is lost.
- Quitting the desktop app parses the quit intent instead of matching a substring, saves window geometry on the way out, and comes back frontmost after a relaunch.
- Restarting the gateway brings back the workspaces it stopped.
- A background task stays drainable after it completes, so a drain that lands on the finish still returns the output.
- Concurrent ADR writes no longer collide on the same number.
- The backup reminder measures against the root font size at measure time, so it follows the UI scale.
- Dependency: rand 0.8.5 to 0.8.7 (GHSA-cq8v-f236-94qc).
## v0.20.1 — 2026-08-03

### Fixed

- Waking the installed iOS PWA no longer fills the screen with error toasts. A wake runs one reconciliation pass, and a dropped connection is reported by the connection dot rather than by a stale-unread-count card.
- A compose draft that fails to reach the engine is queued and re-sent on the next resume or reconnect, so text typed on a phone survives an iOS eviction of the app. Repeat failures collapse into a single card instead of stacking.
## v0.20.0 — 2026-08-03

### Added

- **Max tool calls.** Set how many tool calls the agent may make in one turn, under Settings > Models > Chat & Triggers. Presets or your own number, with an estimate of how long a turn that size can run.
- **Background activity on the brand badge.** The marker beside the Lucidos title now covers any background work and is tappable, opening a status toast naming what is running.
- **Embedding model download progress.** A fresh workspace fetches about 465 MB before vector memory works, and the status toast now shows it byte by byte.
- **The build toast shows elapsed time and the commits the new version will bring.**

### Changed

- Mobile Access recognises a phone already on the tailnet as set up, with no install prompt or setup steps.
- Expose is narrated on the brand badge, uses the current `tailscale serve` syntax with the older form as a fallback, and bounds each attempt with a deadline.
- A Mobile Access failure toast shows the CLI's own error.
- The macOS DMG ships a notarized and stapled app, so Gatekeeper clears it on first launch without contacting Apple.
- Notifications open instantly, from the list already in memory, with a skeleton on a cold push tap.
- The Origin popover on an engine-issued turn names the engine and why the turn resumed.

### Fixed

- A phone held sideways no longer gets a rotate-to-portrait lock. The layout follows the viewport, so most phones get the desktop split in landscape.
- A preference change survives an iOS PWA suspend instead of raising "request cancelled", and writes to one key stay in order.
- A left-edge swipe no longer exits an app that is fullscreen on the installed iOS PWA.
- An auto-update keeps the app's macOS permission grants, and the build refuses an updater payload that is not Developer ID signed.
- A failed update swap reports the failure instead of restarting into a destroyed bundle.
- The update manifest's platform key comes from the built artifact, and it is uploaded only after its payload is present.
- A coding-agent session survives a long silent turn instead of dying with "Stream idle timeout, no chunks received".
- Pausing, creating or deleting a trigger takes effect for the very next request.
- A tool call killed mid-execution renders as unfinished instead of spinning forever.
- A coding-agent worktree is reclaimed only on positive evidence that it is dead, so a slow git probe can no longer destroy live work.
- A workspace whose database provisioning fails transiently, such as the gateway starting before Docker, is retried instead of staying dead.
- An apply refused for incomplete hardening no longer discards the coding agent's commits.
- Streaming turn text no longer double-renders, the applied-changes list scrolls again, and a failed thread-list fetch no longer aborts the resync.
- The command guard reads the real command out of `sh -c '<script>' <args>`.
- An embedding model that does not fit the vector column is refused at load instead of failing every memory write later.
## v0.19.0 — 2026-08-02

### Added

- **Run once.** Fire an existing trigger immediately, off-schedule, from the trigger row or the triggers tool. Not available for paused or event-only triggers.
- **External link target on iOS.** Choose where external links open: Safari, ask each time, or the in-app web view. Set it under Settings > Links.

### Changed

- Mobile Access reads Tailscale state from the machine itself and needs no Tailscale CLI. A CLI is required only for the Sign in and Share actions.
- The Mobile Access page is reachable from a phone, and Get Tailscale opens the App Store or Play Store for the device reading it.
- A coding agent is told what happened to its work between turns: applies, discards, reverts, failed applies and worktree reclamation.
- Every write to credentials, repositories, env vars, models, devices, pinned apps, MCP servers, OAuth accounts, preferences and notifications emits its event.
- A trigger-failure notification links to the trigger.
- The trigger-block message names the blocked command, and a blocked command containing a code fence renders as code.
- Four off-scale font sizes are back on the type scale, and the composer's type scale applies only to the composer.

### Fixed

- Mobile Access publishes a tailnet URL only once something answers on it, and the tailnet HTTP row follows the gateway's network bind rather than tailnet membership, so the page never shows a dead address or the same address twice.
- The Get Tailscale button works outside the desktop app.
- A coding agent parked on an unanswered question survives an engine restart, instead of leaving the thread reading "Working" with a struck-through question card.
- The SDK warms its link-target cache without a theming flash.
- The RC front-door version check retries past Cloudflare POP lag.
## v0.18.5 — 2026-07-31

### Changed

- **A refresh or a notification tap no longer replays the cold-launch animation.** Both loads continue a session the user was in one moment ago, but each arrives as a full document load, so the app answered with the brand launch: the mark building itself, "Opening your workspace…", the gradient, and a 1200ms minimum reveal before anything could dismiss it. Tapping a push on an installed iOS PWA got that treatment every single time, because WebKit implements neither `launchQueue` nor `launch_handler: focus-existing` nor a same-document declarative navigate, leaving a cross-document reload as the only channel that actually carries the deep link. Those documents now paint a quiet cover instead: no mark, no launch ceremony, the app's own flat background rather than the brand gradient, and no reveal floor to wait out, so the reload reads as the app redrawing rather than relaunching. Quiet is not silent, the delayed status still writes, so a genuinely stuck load says so. Three cases deliberately stay launches because each really can be one: the cross-workspace `#thread=` landing hop and a gateway handover can both lazy-start a stopped engine, and a deep link whose value is empty routes nothing at all. The gate mirrors the hash router branch for branch, so what the cover believes and what the app then routes cannot disagree. The cover carries its own light-theme foregrounds, since the delayed status and the escape link out of a stopped workspace are hardcoded white and would otherwise be white on white at exactly the moment boot has given up. The flat repaint reaches both canvas layers, because a fixed inset:0 element never covers the iOS standalone bottom safe-area strip. Reduced motion is honoured on the shorter fade.

### Fixed

- **The pre-paint scripts now read an absolute base href the way the rest of the app does.** `normalizeBasePath` explicitly tolerates a `<base href>` that is a full URL and reduces it to its pathname, but the inline scripts that run before the bundle exists (the anti-FOUC theme resolver and the boot watchdog) each slash-stripped the raw attribute instead. Against an absolute value that yields a slug like `https:/host/myws`, so every per-workspace key the app wrote under `ws:myws:` was invisible to them: the saved theme was not found and first paint used the fallback until the bundle loaded and corrected it, and the boot watchdog namespaced its one-shot retry marker somewhere the app never looks. The same raw comparison also failed to recognise the picker context, whose own no-preference behaviour then did not apply. All three derivations in the document now normalize first and strip second, which is also what keeps a slash-less `~` from being taken for a workspace slug.
## v0.18.4 — 2026-07-31

### Fixed

- **A stopped direct-port workspace is no longer a dead end.** A per-workspace PWA installed on a direct engine port had no way out once that engine was stopped: nothing on that origin lazy-starts a workspace, the gateway is a different origin, and the service worker just replays its cached shell, so the user sat on the boot splash with no explanation and nothing to tap. The splash now offers the workspace's own gateway URL, whose navigation is exactly what makes the gateway start it. The href is built from two stamped metas plus location, so it needs no engine, no network and no bundle, which is precisely what a cached document against a dead origin still has. The engine stamps the workspace slug into the served shell to make that possible, HTML-escaped since the value arrives through an env var and lands in an attribute; a shell predating the stamp falls back to the workspace list. The escape is offered only where it is the only thing that can help, a direct-port document that knows a gateway port, and it replaces tap-to-retry rather than joining it, since a reload cannot start a stopped engine anyway. Revealing it also drops the splash's `aria-hidden`, so the status naming the problem is announced and the focusable link is not stranded in a hidden subtree.
- **A dead engine no longer reads as a running workspace.** `kill -0` succeeds for a process that has already exited and is only waiting to be reaped, so `status.sh --json` reported `engine_running: true` for a workspace whose engine was defunct, with an empty `engine_version` from the health probe in the same call. The control-panel switcher renders that row as a healthy dot with the peer's port, so tapping it sent a direct-port PWA to a dead port, where the service worker serves the cached shell and the boot splash never dismisses. `engine_running` is now reachability and nothing else: the health probe is its only source, which is the only question either consumer is actually asking. The human-readable status output keeps the pidfile as detail, reads it through a new zombie-aware liveness check, and gives an alive-but-not-serving engine its own line instead of claiming RUNNING.
- **A zombie engine is now reaped on the spot instead of blocking respawn forever.** The gateway signal-probed the pidfile pid for a re-adopted engine (the state after a gateway self re-exec, where no child handle is held), so a defunct engine read as alive indefinitely. The respawn decision never culls an alive engine, so the workspace meta-refreshed its boot splash forever instead of being restarted, and nothing else was going to reap it either, since the gateway is its parent. Liveness now answers and repairs in one call, with a `waitpid` scoped to that single pid: reaped-now for our own exited child, alive for a running one, existence probe for a pid that is not ours. Pid 0 is rejected up front, because `waitpid(0, WNOHANG)` means "any child in my process group" and would let the probe reap an unrelated engine and swallow the exit status its own handle is waiting on.
- **A failed boot bundle recovers immediately instead of waiting out the 15 second timer.** Opening a direct engine port with nothing listening left the user on "Opening your workspace…" for the full watchdog window, then again after the silent retry, before the tappable recovery appeared, even though the bundle had already reported it could not load. The inline watchdog now also recovers from the entry module's own error event, caught in the capture phase at window so it needs no attribute on the tag and survives the build rewriting its `src`. It is keyed on the event and never on elapsed time, so a slow but working load is untouched and still owned by the timer, and boot handover removes the listener so a module failing later in the session belongs to the application and cannot reload the page under the user. The single automatic retry now also counts the marker the retry puts on the URL, not just `sessionStorage`, so a browser that refuses storage can no longer reload on a loop.
- **The dark band under the boot splash on iOS standalone is gone.** iOS fills the strip below the layout viewport with the flat base colour of the document canvas, never with the background image, so the base is butted straight against the gradient the splash paints above it. The base was the gradient's 100% stop, but along the bottom edge the gradient has only travelled 62% to 84% of the way there, so the strip read as a distinctly darker band. The canvas is now the gradient's own colour at 70% progress, the mean across that edge, which holds the seam within 4% per channel everywhere along it, on every device, since both progress figures are aspect-independent. The gradient itself is unchanged, and the gateway splash carries the same paint with a test that reads the value out of the `index.html` it already embeds, so the two surfaces cannot drift apart.
- **The engine restart control has one home per install mode.** A packaged install (the macOS app or the headless tarball) ships its binary and has no source, so its restart only respawns a service, yet System > Overview offered it as "Rebuild & Restart" and named an operation that cannot happen there. Dev keeps "Rebuild & Restart" under Overview > Maintenance, where the restart really does re-run the dev script, and packaged gets a "Restart Engine" row under System > Debugging alongside the other diagnostics. Returning the home rather than handing each site a boolean makes "both render" and "neither renders" unrepresentable, both sites call the same shared confirm-and-restart path so the dialog cannot drift between them, and the settings search index gains a packaged-only gate so a dev search never offers a Debugging row that does not render there.
## v0.18.3 — 2026-07-31

### Fixed

- **A packaged client notices a new release without being restarted.** The app updater was the one update surface that window resume did not reconcile: the client re-checked the service worker, the frontend build id, the engine build state and the unread set on focus, but never the app release. A 0.18.0 client started at 08:54 ran its single startup check while 0.18.0 was still current, then sat there while 0.18.1 and 0.18.2 shipped, with the next unattended check not due until 14:54. It reported itself up to date all morning even though a manual check from Settings resolved 0.18.2 immediately. The release recheck now joins the other resume rechecks, throttled to one network round trip per five minutes because focus and visibilitychange fire on every window switch, and the background poll interval drops from 6 hours to 1 hour. The throttle stamp is only taken when a check actually reaches the release host, so a resume landing mid install cannot defer the next real check. A guard test pins the resume reconciliation set so a future refactor cannot quietly drop a surface out of it again.
- **The boot splash stops resizing and replaying itself during launch.** Both splash surfaces sized themselves in rem, but they live in documents with different roots: the app document scales with the user's UI scale preference while the gateway splash is an isolated document at the browser default. At 137.5% scale the same cap painted a 330px mark in the app and a 240px one on the gateway, with the status line 9px further down, so the brand jumped the moment the workspace document took over on the same URL. The mark also rebuilt its reveal at every hop, since the gateway replays it on each 2 second refresh and the app document then played its own on top. Splash geometry is now pinned in px and defined exactly once: the gateway lifts the stylesheet and the mark markup out of the app document verbatim at compile time and overrides only the four things it genuinely needs (a wrapping status line, no idle animation under its meta refresh, a tappable escape link, its own label). A per tab handover marks the mark as already standing, so it is revealed once per boot and then simply stays, which also drops the minimum reveal floor that was holding a finished splash up for an extra second at the end of a slow cold boot.
- **The gateway build id counts the embedded app document.** With the splash markup now compiled into the gateway, the build id was still derived from the gateway crate and the lockfile alone. An uncommitted edit to the app document rebuilt the binary with a new splash under the running gateway's old id, so the workspace picker's reload status reported no update and kept serving the old splash with no signal. The embedded file is now named once and feeds all three consumers: the rebuild trigger, the dirty diff pathspec and the no git fallback hash.
- **The gateway escape link renders in the product typeface again.** Consolidating onto one stylesheet dropped the page level font declaration, and the shared sheet scoped the stack to the status line only, so "Back to workspaces" inherited the browser serif. The typeface, size, line height and letter spacing are now declared once on the splash container and inherited by every line on both surfaces.
## v0.18.2 — 2026-07-31

### Fixed

- **Uninstalling on macOS actually removes the LaunchAgent.** `launchctl bootout` is asynchronous: it returns 0 the moment launchd accepts the request, not when the job is gone. The gateway ignores `SIGTERM` by design, so launchd has to wait out its exit timeout and `SIGKILL` it, and for that whole window (measured at about five seconds) the job is still bootstrapped. The uninstaller took the exit code as the answer and reported "Stopped launchd agent" over a job that was still registered and, because the agent carries `KeepAlive`, still respawning a gateway until the next logout. Both wrappers now decide by observing the domain rather than by reading an exit code, bounded by a timeout that `LUCIDOS_LAUNCHD_TIMEOUT` overrides.
- **Re-installing over a running instance no longer silently unregisters the service.** The load path booted the old job out and bootstrapped immediately, into a domain that still held it. launchd refused with `Bootstrap failed: 5: Input/output error`, the legacy `load -w` fallback also failed while exiting 0, and the loaded check still saw the old job, so the install reported success. Seconds later there was no job in the domain at all, which meant an upgrade or a `--port` change left the LaunchAgent gone until the next login. The unload now completes before the bootstrap, and a failure to unload is reported instead of being bootstrapped on top of.
- **A failed stop no longer leads to destructive follow-up steps.** With the failure finally detectable, three downstream assumptions were wrong: the uninstaller killed engines that a live `KeepAlive` gateway just respawns, `--purge` deleted an instance's data while its Postgres was still writing to it, and `--all --purge` deleted the shared runtime out from under the running binaries. Each of those is now skipped when a service could not be stopped, with the manual `launchctl bootout` command printed so the user can finish the job.
- **The uninstall summary no longer claims a purge it did not perform.** A refused purge printed the same "uninstalled + purged" banner as a completed one. It now names the data that is still on disk and why.
## v0.18.1 — 2026-07-31

### Changed

- **OpenAI Responses API requests are sent with `store: false`.** Every request now opts out of server-side response retention rather than relying on the API's `store: true` default, so prompt and response bodies are not kept by the provider. Full conversation history is rebuilt locally as `input` on each call, which is what makes the opt-out possible.
- **Running Lucidos from source is documented as a development guide** rather than as an installation route, so the quickstart stays about installing and a separate develop page covers the source workflow.
- **The two front doors are gated on serving the same routes.** A piped `curl | sh` fetches its helper libraries back from whatever origin served it, so an origin that quietly stops serving one of them turns the next install into "execute a web page". A route-parity harness now checks production and the release candidate against the same expected route set, and the front-door CI jobs fail when the two diverge instead of only when production breaks on its own.
- **The Access service token is sent only to the origin that is gated.** The front-door jobs attached the release-candidate credential to every origin, including the public one that does not want it, and reported having sent a token in cases where none applied. The precheck now scopes to any gated origin and says what it actually did.
- **A front-door check waits for the release assets instead of racing them.** The job downloads the per-platform tarball from the release being tested, and a check dispatched before the upload finished failed on a missing asset after burning its full health timeout. It now waits for the assets within a bounded window, and a genuine download failure is reported as such rather than as a timeout.
- **A release-candidate origin arriving on a dispatched run is refused.** The candidate front door is owned by the `rc/**` push arm and is payload-checks-only, since the tarballs for an unreleased version do not exist yet. A dispatch naming that origin is a caller error, and absorbing it into a passing run would hide exactly what the job exists to surface, so it is rejected before any fetch.
- **Dependabot no longer retries a security update that cannot resolve.** The advisory range for `glib` is bounded below the versions this tree can move to, so the update was reopened and failed on every run. The ignore is scoped to that unresolvable range rather than to the package, so a future advisory fixed inside the reachable range still alerts.
- **`lettre` updated from 0.11.19 to 0.11.22.**

### Fixed

- **The Tailscale IP is detected in the packaged app.** The probe resolved the binary by name, which works in a terminal but not inside a bundled `.app` where the user's shell PATH is absent, so Network access came up without the tailnet address. It now resolves by path across the known install locations, skips the macOS GUI binary that is not a CLI, and logs a detection failure when a real CLI was found but did not answer.
- **The workspace picker's Network access popover opens on the saved bind.** It previously opened on a default and settled onto the stored value a moment later. The Save control is also sized to the longer of its two labels so the button no longer resizes as its state changes.
- **The Browser row in the menu drawer follows the experimental in-app browser setting.** The row was always present regardless of the toggle. A single availability gate now governs the drawer row, the settings entry and the navigation path, so the three cannot disagree.
- **The README no longer uses em dashes.** Thirty-three of them were rewritten as commas, colons, parentheses or separate sentences, with no content added or removed.

### Removed

- **The dead documentation deploy workflow.** Documentation publishes from the maintainer's machine off a workspace trigger, not from CI, so the workflow could only ever fail. ADR 0031 records why deploys do not run in CI: the available credential form carries broader zone permissions than a CI job should hold.
## v0.18.0 — 2026-07-31

### Added
- **A packaged update narrates itself, and the download can be cancelled.** Clicking *Update & restart* on a packaged install produced nothing visible for as long as the whole update took (a ~100 MB download, a signature check, a bundle swap and a service restart), then the app vanished and came back. Tauri hands the updater per-chunk byte progress and a download-finished hook; both callbacks were empty, so the entire run was one silent `await`. Every step now reports itself over an `app-update-progress` event and the page narrates it live: *Checking for updates*, *Downloading* (with bytes transferred and a real progress bar), *Verifying*, *Installing*, *Restarting background services*, *Relaunching*, in the toast and in Settings, System, which share one derivation so they cannot disagree. The download is **cancellable**, since nothing is on disk until it is verified, so abandoning it costs nothing and the update stays on offer; the phases past that point withhold the affordance rather than offer a cancel that could not work. A failure names its reason instead of leaving a spinner, and a download whose size the server does not declare shows bytes with no fabricated percentage. The bundle swap also moved off the async runtime worker, so the progress it reports keeps flowing while it runs.
- **A coding agent no longer asks permission to write inside its own worktree, and its session allows survive a restart.** Every file write a coding agent made raised a permission card, including writes to the isolated worktree the session was created to edit, which is the one place its writes are already contained. Those are now auto-allowed (with symlinks resolved first, so a link out of the worktree is still a real write outside it and still asks). Separately, a session allow the user granted lived only in engine memory, so an engine restart mid-session made the agent re-ask for everything it had already been permitted. Allows are now rehydrated per thread on boot.
- **An app can open a file from a registered repository in the preview pane.** `lucidos.ui.navigate('file', ...)` previously reached only workspace data paths. It now also accepts the repo-encoded form `repo:<repoId>:file:<repo-relative path>`, read at the clone's current `HEAD`, and the preview binds itself to that repository so the Files panel and the changed-files sidebar stay on the same repo. A malformed `repo:` string is treated as an ordinary artifact path rather than an error.

### Changed
- **The mobile thread pane header is bracketed by its two drawer controls.** The thread drawer toggle leads the row and the hamburger moves to the far trailing edge, so the two drawers mirror each other across the header and both stay one tap from the conversation: reaching Settings no longer costs a swipe over to the content pane. The menu drawer now slides out from whichever edge its opener sits nearest, so a tap on the trailing hamburger produces a panel from the right instead of one crossing the whole screen. The content pane header keeps its leading hamburger and its left-side panel, and desktop is pinned left, since its panel emerges from the split divider rather than a viewport edge.
- **A status-filter view offers "See all statuses" when it has rows, not only when it's empty.** Filtering the thread drawer to Needs attention, Review, Running or Drafts narrows it to a handful of threads, and the shortcut back to the unfiltered list only appeared under the "nothing here" message. So the moment one thread landed in the filter, the exit vanished and the way out was back up in the filter control. The same link now closes out the list in both states, at the end of what the user just read.
- **`make lint` now fails on unformatted Rust, and the tree was swept clean once so it can (ADR 0030).** Formatting was pure convention: `make fmt` existed, nothing ran it, and 424 of 614 tracked `.rs` files (69%, 1,940 hunks) had drifted. A new `lint-fmt` target runs `cargo fmt --all --check` between the ShellCheck and clippy passes, ordered cheapest-first like the rest of the gate, and points a failure at `make fmt`. Because `/harden` Phase 4.5 already routes `.rs` and `Makefile` diffs to `make lint`, every future change picks it up with no further wiring, and nothing was added to GitHub Actions. The sweep landed as its own commit containing no hand edit, so `git blame --ignore-rev` can skip it wholesale. Three decisions are recorded in the ADR rather than left to be rediscovered: there is deliberately no `rustfmt.toml` (on a stable channel rustfmt warns and continues on a nightly-only key, so a config file would read as active while being inert, and the existing toolchain pin already makes stock defaults reproducible); the CLI codegen emitter now formats its own output, because a tracked generated file is squeezed between its staleness test demanding byte-equality with the emitter and the gate demanding rustfmt-cleanliness, and neither `ignore` nor `#![rustfmt::skip]` can exclude it on stable; and this one cargo call carries no `--locked`, against ADR 0020's blanket rule, because `cargo fmt` rejects the flag and resolves no dependencies. One consequence worth knowing: a toolchain bump that moves rustfmt's output now reds the gate, so such a commit may have to carry a reformat.
- **Uninstalling is a real CI gate now, not a teardown attempt whose result was thrown away.** The front-door jobs ran an uninstall at the end of the run and ignored what it did. Four asserting rungs were added on both Linux and macOS: the advertised `install.sh | sh -s -- --list` delegation actually fetches `<origin>/uninstall.sh` and gets a script rather than HTML; the direct `uninstall.sh | sh -s -- --list` path (the only leg that exercises the dash re-exec) does the same; `--uninstall --all` leaves the data directory intact while booting the agent and deleting the service definition; and `--all --purge` removes the data directory and the shared runtime. The payload sniff in rung 1 covers `<origin>/uninstall.sh` and its self-URL pin too, which closes the client half of a soft-404 on that route.
- **The release candidate front door moved to its own gated origin.** `/rc` is published in the same Cloudflare Pages deployment as production (a deploy replaces the whole manifest, so a separate RC publish would take the real front door down) and the installer copy served there is pinned at the RC URL, so its helper libraries resolve under `/rc/` instead of silently verifying main's installer.
- **A release tag now names the real main-line commit locally, and the stripped public commit on the mirror (ADR 0029).** The mirror tag is pushed by SHA, so no local ref is created or clobbered, and only a tag the run actually settled is eligible to be published to `origin`. The release also lands its own bump on live main (cherry-picking when main moved during the build) instead of warning and continuing, which is what previously let a successful publish leave the site serving the previous version.
- **Em dashes are banned in this repo, enforced at write time and at harden time.** A shared scanner backs a write-time hook and a diff-scoped gate; it is added-lines-only on purpose, because roughly 29,000 existing lines carry the character and a whole-tree scanner would be switched off within a day. U+2013 EN DASH is deliberately not banned, since it is legitimate in numeric ranges.
- **The docs say what a stranger would actually need.** The quickstart leads with the signed, notarized macOS `.dmg` rather than burying it under the one-liner; install instructions point at `lucidos.dev/install.sh` instead of raw GitHub URLs; `--purge` states what it destroys and stops naming a file piped users do not have; the README no longer claims a bare `uninstall.sh` is a dry run (it uninstalls); and `PRIVACY.md` now discloses the desktop app's update check against GitHub Releases.

### Fixed
- **The iOS PWA no longer wakes up to a blank panel, or flashes white when an app opens.** Two halves of the same WKWebView compositing problem, both surfacing only in the installed PWA. First, `.content-pane-body` is an `overflow-y: auto` scroll container, so WebKit gives it its own compositing layer, and a backgrounded PWA (the phone locked) leaves that layer frozen on a stale or empty backing texture: the panel is fully rendered and laid out in the DOM, and nothing is on screen. Waking changes no signal, so no render produces DOM changes and only an explicit repaint can un-blank it. The same root cause was already fixed for the thread body; this container was the surviving half. The repaint is now wired to the shared page-resume signal (`pageshow`, `focus` and `visibilitychange`, since iOS often restores a PWA through `pageshow` alone) and fires **on resume only**: a per-view version was tried and reverted, because the recovery nudge writes `scrollTop`, which the mobile header's hide-on-scroll listens to, so every panel switch moved the header and forced repeated synchronous layouts while the incoming view was still mounting. It also skips a mounted app-ui iframe, which composites out of reach anyway and whose pseudo-fullscreen panel would be snapped back to the pane's box by the transform. Second, an iframe with no document yet paints its base canvas, which WKWebView fills **white**, so on a dark theme every app open flashed white until the app's stylesheet and `/api/v1/sdk-prefs.js` (a second request, and the thing that actually applies the theme) arrived. Neither gap is reachable from the host, so the host now covers the frame with an opaque themed surface from mount and crossfades it out on load, with a three-second fuse so a hung frame reveals whatever it managed to paint rather than staying covered forever. An app switch re-covers.
- **`install.sh` and `uninstall.sh` refuse to execute a fetched payload that is not a script.** Both scripts fetch helper libraries and, when piped, re-fetch themselves, then hand the result to `exec bash -c` or to `.` (source). `curl -f` cannot catch the dangerous case: a static host that answers an unknown path with its landing page returns HTTP **200**, so a missing library became "execute a web page as shell" on the user's machine. All four sites now sniff the payload for a shebang before running it and refuse with a clear message otherwise.
- **Rapid chat messages keep the order they were sent in.** A second message dispatched before the first had been acknowledged could overtake it, because the ordering slot was claimed at POST time rather than at call time. Sends are now serialized per thread, a follow-up's `MessageReceived` is persisted before its POST is acknowledged, and a lone send is no longer deferred by a microtask.
- **A link in a previewed file navigates inside Lucidos instead of hijacking the pane.** Opening an HTML artifact renders it into a `srcdoc` iframe, and such a document has no URL of its own: it resolves every relative and fragment href against the *host page's* URL. So a report's own table-of-contents link `#section` resolved to `https://<gateway>/<slug>/#section` and the iframe dutifully loaded the entire Lucidos app into the content pane; relative image and stylesheet refs reached for the app shell the same way. Preview iframes are same-origin, so link clicks are now bridged from the iframe's own document the way keyboard chords already were: an in-page anchor scrolls the preview, a `thread:` link opens the thread, an artifact, app or panel link routes through the host (which is what gives it a content-pane history entry), and an external link opens in a new tab. The document is also stamped with a `<base>` pointing at the artifact's own folder, so its relative assets resolve to its siblings. Markdown artifacts got the same routing: they render into the host document, where a sibling link like `notes.md` resolved to `/<slug>/notes.md` and reloaded the whole workspace through the SPA fallback.
- **A cross-workspace thread link works on the first click.** Opening `[title](thread:<ws>/<uuid>)` sends the browser to the peer workspace at `#thread=<uuid>`, and the gateway lazy-starts that engine on the same request. The landing page consumed the hash *before* trying to open the thread and gave up after one attempt, so the bootstrap that raced a still-booting engine lost the only record of where the user wanted to go. The second click, hitting a warm tab, worked. The hash is now consumed only once the thread is actually focused, with bounded retries while the peer engine comes up; a thread that genuinely is not there says so instead of retrying.
- **A registered Python script runs from its real path, not from a copy.** Trigger and app scripts were executed out of a staging copy, so `__file__`-relative state and data directories resolved somewhere nobody had created, and a script would report "no X recorded" for something plainly on disk. Scripts now run in place, with the workspace root as the working directory.
- **An update that is cancelled at the last moment stays cancelled.** An accepted cancel could lose a race against the download-to-install commit and install anyway.
- **Screenshots captured inline from an app no longer bloat every read of the thread.** The capture payload is stubbed on every read path rather than only the first, and the model-facing and persisted forms of a tool result are now derived separately instead of one being a copy of the other.
- **The boot splash stops claiming "Workspace not started" when nothing failed.** The message was shown on a slow probe as readily as on a failed one.
- **An overflowing thread title is ellipsised in the desktop header** instead of running under the action icons, and it recovers its full width when the pane widens.
- **Settings, Accounts rows match the Apps and Triggers geometry.** The action buttons bottom-align at any pane width, a long OAuth provider name wraps, and the empty scopes span is gone.
- **The installer records its instance port marker on a foreground launch too,** not only when it registered a background service, so `--list` reports the instance either way.
## v0.17.0 — 2026-07-29

### Added
- **A workspace that cannot boot now says why, instead of spinning forever.** A downgrade onto a database a newer Lucidos had already migrated made the engine exit on every spawn; the gateway respawned it five times, marked the workspace unhealthy, and showed "Workspace starting…" for four minutes before settling on "This is taking longer than expected." The actionable cause lived only in `engine-service.err.log`. The engine now classifies the migration error, builds a message naming the version gap (how many unknown migrations, and the newest one), and reports it to the gateway before exiting. The gateway stops respawning — a version mismatch never heals by retrying — and renders the message on the splash with no auto-refresh and an escape link.
- **A packaged update is reachable after the toast is dismissed.** The remedy for a too-old app is "install a newer version", and the only surface offering it was a transient toast. Settings → System could never show one in a packaged build: it read the engine's `/health` version, which the engine derives from the repo, so a packaged install reported `unknown` and the comparison always came out false. That surface is now fed from the packaged updater, with a persistent System notice and **Update & Restart** / **Check for Updates** buttons. The update check also runs on every workspace mount rather than only the first of a client process, so a release cut mid-session no longer stays invisible until a full quit.

### Changed
- **The first-run workspace suggestions are now "personal" and "work".** The name-your-first-workspace chips offered "home" and "team" — "team" implies a shared, multi-user space that Lucidos does not provide, and "home" reads as a location rather than a purpose. The new pair splits on the axis people actually organise by, and neither one over-promises.

### Fixed
- **A mid-turn message is treated as an interjection, not a plan override.** Every human message injected into a running turn was wrapped in "USER CORRECTION — prioritize this over your current plan", so a bare "status?" read as a course change: the agent answered and ended the turn, dropping the work in progress. The framing now states both readings and defaults to resuming — answer, then carry on in the same turn — while a genuine redirect still overrides.
- **A re-processed orphan is no longer told to resume work that already ended.** The same framing reached a second caller that builds the *opening* text of a brand-new turn, where "carry on with the work you had in progress, in this same turn" named a turn that had already terminated. Delivery is now explicit at each builder; the new-turn path reports only when the message was sent and carries no resume directive.
- **A transient construction fault no longer kills a workspace permanently.** Reporting a boot failure stops the gateway respawning, so the catch-all around engine construction turned a Postgres that wasn't ready yet — or a connection dropped during schema init — into a dead workspace the supervisor would otherwise have recovered. Only errors that re-run identically forever are reported as terminal; everything else keeps its retry.
- **The updater no longer clobbers a version it didn't set.** In a Tauri dev client the update check no-ops to null, which is indistinguishable from "up to date" — assigning it blindly wiped the version read from the engine, and the two fought on every poll. The notice, the button label and the button's action now share one source, so they cannot disagree, and the terminal-failure splash no longer leaves the tab title claiming "Starting…" while the page says the workspace cannot open.
- **An install without a Lucidos source checkout no longer offers to edit the platform.** A packaged or headless install had nothing in its chat context saying it has no source tree, so the agent claimed it had read engine source, spawned a coding-agent session that *succeeded*, and told the user to Apply and rebuild — the session had branched the user's own workspace git and called it platform source. The spawn path now refuses at both the tool and the session, and the system prompt splits into two variants selected by whether a checkout is actually present, so the model learns the limit in-turn instead of narrating a capability it doesn't have. The refusal is scoped to *local* spawns: routing platform work to another workspace whose engine does run from a checkout stays available, and tests pin both directions.
## v0.16.0 — 2026-07-29

### Added
- **Provider-native web search** — `web_search` now resolves over the configured provider set instead of a single hardcoded backend. Adds a `WebSearchProvider` trait, a fallback chain, and three backends (Anthropic, OpenAI, Vertex/Gemini grounding), each pinned to a model its own provider actually serves. Prompt and result formatting are shared across backends, and `max_uses` is decoupled from the chain. (ADR 0023)
- **Per-model context windows** — every builtin model now declares a *verified* context window, editable in Settings, and that value drives the context-trim budget instead of a guess derived from the model id. The reported token total counts tool schemas, and truncation is surfaced honestly as *trimmed*. Bare Claude rows stay on the prefix map since they send no 1M beta header.
- **GPT-5.6 (Sol / Terra / Luna)** — added to the chat model picker and the Codex coding-agent `/model` picker, with the `max` reasoning tier enabled for the family. Max effort is validated server-side and filtered by the selected model in the picker.
- **Claude Opus 5** — added to both the chat and Claude Code pickers and set as the default model (via `cc-settings.json`).
- **Built-in model-provider proxies** — `vertex`, `openai`, `openrouter`, `anthropic`, and `local` are available as proxy targets out of the box, so an app can call a model provider without hand-registering it in `apis.json`. Bearer credentials get the Anthropic OAuth beta header injected automatically.
- **Packaged system-knowhow** — `system-knowhow/` ships as the 7th runtime resource and is resolved at runtime via `LUCIDOS_SYSTEM_KNOWHOW_DIR`, so a packaged install has the same authoritative reference docs as a source checkout.
- **Codex parity work** — Codex sessions stream reasoning summaries, load `CLAUDE.md`, validate the effort tier, surface plan-tool progress over the app-server protocol, and map slash commands to playbook files.
- **Chat threads auto-resume after a version switch** — a chat *or* trigger thread interrupted by *Switch to new version* now resumes on the next boot, matching what coding-agent threads already did, instead of leaving a device-attributed abort and a manual Continue button. A dedicated boot pass handles them (the switch teardown always lands a terminal event, so the orphan sweep structurally can't see them), and the shared cause gate now requires an engine-shutdown cause so a user Stop is never mistaken for a switch.
- **Stranded-Apply visibility** — a frontend Apply that can't reach the served dist is now surfaced instead of failing silently, and the engine warns once when the dist it's serving is pinned to a coding-agent worktree.
- **Zero-file Apply refusal** — Apply is refused server-side on a change whose branch diff has gone empty, and such a change is reconciled (without discarding its siblings) rather than left blocking the panel.
- **Compose-clear announcements** — every clear of the compose box is announced to other devices, whether it came from an ordinary send or from answering a question card.
- **App Store Connect API key notarization** — the release build prefers an ASC API key over an Apple ID app-specific password when one is configured, so notarization runs cleanly headless.
- **One source of truth for the release version, enforced** — `RELEASE` is the only place the version is written by hand; everything else derives from it (the build reads it, the release flow rewrites `install.sh`'s baked constant, the site publisher pins the landing page's download links at publish time). A new offline suite scans the tree for a version literal nothing keeps in sync, pins both halves of the `install.sh` mechanism so deleting the substitution fails immediately rather than at the next bump, and flags prose that announces which release line the project is on. Historical narration stays exempt — the changelog, plans, and ADRs are correct precisely because they don't move. Phase A runs the suite against the worktree after the bump, so a stale literal fails the release instead of reaching the mirror.
- **Resumable notarization** — the release build submits with `--no-wait` and writes a resume handle (submission id, DMG path + sha256, source commit, submit time) to disk *before* it starts polling, so losing the waiting process costs a poll instead of a full rebuild. A build-grade run auto-resumes when a handle matches the DMG on disk, `--adopt-submission <uuid>` picks up a submission that's already in flight, stapling is idempotent, and the handle is cleared once staging succeeds. Backed by a pure, offline-testable state library (`scripts/lib/release_notarize.sh`) with atomic writes and a checksum gate that refuses to resume against a changed DMG.
- **A re-fold on unchanged bytes no longer burns a notarization submission** — one release spent 16 Apple submissions in two days, several of them on byte-identical compiled input, because *any* movement on `main` forced a rebuild + resign + resubmit. The build now fingerprints only what actually reaches the bundle — the seven compiled-input paths (`crates`, `Cargo.lock`, `Cargo.toml`, `packages`, `package.json`, `package-lock.json`, `system-knowhow`) — and Phase A consults that gate before rebuilding, reusing the already-notarized DMG when every hash matches. A docs-, plans-, or `scripts/`-only re-fold is now free. The fingerprint deliberately excludes the driver scripts themselves (a one-word comment edit in `build-dmg.sh` was enough to flip a coarser hash and re-break the very case this fixes); a separate recipe fingerprint tracks those, and both are recorded in the staging manifest. Also fixes a concurrent-poller miscount where a resume's own subshell was reported as a second poller.
- **Shellcheck gate on every shell script** — `scripts/lint-shell.sh` + `make lint-shell`, wired into `lint` so `make check` covers it. Discovery is `git ls-files '*.sh'` rather than a hand-written path list, so a script added in any directory is gated the day it's committed. Fails closed on all three ways it could be disarmed (no shellcheck on PATH, empty discovery, a silently-skipped file). The sweep that brought the tree clean fixed the findings at the source rather than suppressing them — the `SC2155` masking bugs below were found by it.

- **A release no longer waits on Apple** — notarization verdicts have taken anywhere from ten minutes to fourteen-plus hours, and a wedged notary used to hold the whole release. `--defer-notarization` stages the DMG *unstapled* (recorded as `notarized: false` in the staging manifest) and publishes with a "notarization pending" banner on the release body, so the in-app update, the headless tarball and `curl … | sh` — none of which touch Gatekeeper — ship immediately, while the download button deliberately stays on the previous notarized DMG. Against a submission already in flight it stages without polling at all, which rescues a Phase A stuck on a slow verdict with no rebuild. `--attach-notarized` then staples, swaps the published asset in place, removes the banner, and fires the clean-machine DMG gate against the published tag. (ADR 0027)

### Changed
- **The release candidate is the published artifact** — the rc branch is now a pre-stripped, validated tree that gets *promoted* to `main` rather than rebuilt at publish time, so the commit CI gates is byte-identical to the commit that ships. Adoption requires a parentless candidate (a candidate carrying ancestry would push every reachable object while the private-data scan only inspects the tip tree), and the promote step re-asserts that at the irreversible push. The exclusion list, the `WORKSPACES.md` stub, and the fail-closed private-data scan are now single-sourced. (ADR 0024)
- **`bash_output(wait_secs)` actually blocks** — the drain now waits the full requested window (server-side) rather than returning immediately, and reports a real elapsed clock. A user message cuts the block short so their follow-up isn't stuck behind it.
- **Background tasks report their true exit status** — the completion summary no longer masks a failing pipeline stage; `pipefail` semantics are documented (rightmost failing stage wins), `128+signum` exit codes are decoded, and a trigger script killed by a signal is named as such.
- **Accent palette** — `--accent-yellow` retheming to a muted sand, with the "notable state" role split off into its own `--accent-notable` token. Dead `--initiator-*` tokens dropped.
- **Client build id** — the frontend shows the client's *own* build id instead of the engine's frozen CalVer, and launch binaries are published per build variant rather than through cargo's shared uplift path. (ADR 0022)
- **Dev stack refuses worktree pinning** — a dev stack can no longer be launched from a coding-agent worktree, in either the shell path or the gateway; the opt-out no longer buys a machine-global gateway. (ADR 0021)
- **Picker tooltips** — native `title` tooltips replaced with the shared `data-tooltip` system, keeping the status dot's accessible name intact.
- **Rules layout** — the monolithic scripts rule split into `dev-runtime` and `build-release`, with path-scoped frontmatter so rules load conditionally; env-var reference moved out of `CLAUDE.md` into a lazy-loaded skill.

### Fixed
- **The packaged macOS app could not talk to its own backend** — upgrading to Tauri 2.11 broke every webview→Rust IPC call in the shipped bundle, and with it native notifications, window dragging, and the durable device id. The packaged window loads over the gateway rather than `tauri://localhost`, which Tauri treats as a *remote* origin; 2.11 tightened the ACL so a remote origin gets no permissions unless a capability names it explicitly. Every command failed the ACL check, the startup health probe never completed, and the reload watchdog re-loaded the WKWebView every 60 seconds forever — a loop that could not possibly help, since the origin's permissions don't change on reload. Fixed with an app-level ACL manifest (`allow-app-ipc`) granting the gateway origin its permissions, a watchdog that backs off when reloading demonstrably isn't the cure, per-command IPC health tracking, and an honest report to the engine log when the bridge is down. (ADR 0028)
- **New installs get persistent notification banners** — macOS assigns every newly-authorized app the *Banners* alert style, which auto-dismisses after about five seconds, so a Lucidos notification could vanish before it was read. The bundle now declares `NSUserNotificationAlertStyle = alert`, the same first-launch default request Chrome and iMovie ship, so a fresh install starts on the sticky *Alerts* style. It is only a default: once macOS has created the app's Notification Center entry, the user's own choice in System Settings wins permanently.
- **A cron slot no longer double-fires after a restart** — catch-up re-ran a slot that had already run, and the recorded run time is now resolved in Rust rather than in SQL.
- **The repositories picker no longer sticks on a cancelled read** — an aborted fetch was latched as a permanent error state instead of a transient one.
- **Codex can write to the workspace again** — the sandbox denied `EPERM` on `data/` because the writable root was derived from the git common dir rather than the actual workspace data directory. The root is now canonicalized, and a `data` symlink may *relocate* the sandbox hole but never widen it.
- **An auto-resume after a version switch no longer wedges the thread** — a confirmed resume attach was being classified as stale, and a failed continuation left no recovery arm. Losing the spawn race to a live session no longer settles it.
- **The real published front door is tested in CI** — a job now runs the exact advertised command (`curl -fsSL https://lucidos.dev/install.sh | sh`) against the live origin rather than a checkout, with the origin parameterised so an RC can be gated the same way. The installer also rejects a soft-404 HTML payload before sourcing a fetched helper lib, instead of executing a web page.
- **Autocorrect on prose text fields** — a JSX `autocorrect="off"` inverts to *on*; the shared stamp now owns the attribute.
- **The focused pane paints through the reclaimed macOS title-bar band** instead of stopping short of it.
- **Rust toolchain pinned** (1.94.1) so a "clean" build stops drifting between machines.
- **The `curl … | sh` install one-liner works again** — the baked `LUCIDOS_DEFAULT_VERSION` fallback in `install.sh` had been stranded at `0.14.0` while `RELEASE` moved on, and it is precisely the value a *piped* install resolves (no checkout ⇒ no adjacent `RELEASE` file to read). Since v0.14.0 predates the headless tarballs, the advertised one-liner was not installing an old version — it was 404ing. The release flow now rewrites that constant in the same step that bumps `RELEASE`, failing loudly if the substitution doesn't match, and a guard test asserts the two can never diverge by hand.
- **…and it works on Linux too** — a real clean-machine test (fresh `ubuntu:22.04`, the exact command the README and landing page advertise) found two more blockers, neither visible to CI, which runs the installer from a checkout and so never takes the piped-dash path. On Debian/Ubuntu `/bin/sh` is dash, so the bash re-exec guard re-fetched the script from `LUCIDOS_INSTALL_URL` and exec'd *that* — discarding the copy the user piped and re-resolving its own baked version, so piping lucidos.dev's 0.15.0 installer actually installed 0.14.0. The guard now pins the resolved version across the re-exec (an explicit `LUCIDOS_VERSION` still wins) and the constant moved above the guard so both branches read the same value.
- **A wedged coding-agent thread no longer needs an engine restart to clear** — membership in the session table was treated as liveness by every reader, but only the run loop ever set `process_exited`. A run future *dropped* rather than completed (cancelled caller, aborted task) left the entry behind with that flag false and its receiver gone. One such phantom fooled three readers at once: worktree cleanup logged "live agent session active" forever, the chat fast path sent follow-ups into a dead channel, and the resume guard refused every follow-up with "A coding agent is already running for this thread". Liveness is now derived (`!process_exited && !msg_tx.is_closed()`), and a drop guard marks the session dead on every abnormal exit path.
- **Apply's merge session survives the browser disconnecting** — Tier 2 awaited a whole coding-agent merge inline, so the session's lifetime was the caller's HTTP future. iOS Safari dropping the connection 72 s into a conflict resolution killed the merge mid-tool. Tier 2 now detaches through the same guarded spawn Tier 1 and Tier 3 already use and answers immediately with a conflict result. Hardening review caught two consequences of the detach and fixed both: the orphan-sibling reconcile (which stops a stale pending change on another branch from blocking Archive) now runs inside the spawned task, gated on the change actually reaching `applied` so a failed or handed-off merge can't discard a newer sibling's work; and the drop guard no longer claims an abort for an entry it doesn't own.
- **An interrupted thread keeps its red status dot** — an interrupted Lucidos Agent thread kept its `failed` status; an interrupted Claude Code / Codex thread silently lost it. Both channels emit the same abort, so the abort was never the problem — what lands *after* it was. The restart teardown emits the boundary abort while the subprocess is still alive, and the duplicate-terminal suppressor doesn't stop the activity stream, so tool results still arriving milliseconds later (~13 ms in the observed trace) hit the "bump back to running" arm and overwrote the verdict. The chat mirror of the same bug: the shutdown sweep emitted an abort with no request id, so the loop's own cancel couldn't be paired with it and the phantom cancel walked the red dot back to idle — `thread_summaries.status` is last-write-wins, which the old docstring's "abort takes precedence" claim only held for the exchange label.
- **A test run can no longer kill the machine's live dev engine** — the host-pid kill guard read caller-owned state in both its arms, so any caller could switch it off. `ports_test.sh` does exactly that as part of being a well-behaved sandboxed test (it unsets the host/frontend pid vars and points `HOME` at a temp dir), which left the live engine invisible to the guard, matching `*lucidos-engine*` on cmdline, and reachable by the stale-port reclaim. It died twice on 2026-07-28. Two additive arms were added — the guard gains reasons to refuse, never reasons to permit — and the test suite is now structurally unable to signal the host. (ADR 0025)
- **Silently-swallowed command failures in the workspace scripts** — nine `SC2155` instances of `local x="$(cmd)"`, where the exit status is `local`'s rather than the command's, so a failing `date` / container lookup / database-URL resolution passed silently. Split into declare-then-assign. Three globals with zero readers anywhere in the repo were deleted rather than suppressed.
- **`CONTRIBUTING.md` describes how contributing here actually works** — the guide read like a conventional upstream repo, so a contributor had no way to know this repository is a *published mirror*: `main` is one parentless commit force-pushed per release, successive releases share no history, and a PR is *imported* (squashed onto the previous tag with a `Co-authored-by:` trailer, then closed with a link to the release that carries it) rather than merged. It now says so up front, including what to do with a fork whose ancestor no longer exists, and notes that CI never runs on PRs so contributors should report what they ran locally. Two documents separately announced that Lucidos was "currently on the 0.9.x line" — a claim nobody re-reads at release time; both now point at the newest tag.
- **A first-run install no longer races the PostgreSQL init server** — a clean-machine install could die at "Creating shared PostgreSQL database" with `psql: connection to server on socket … failed`, seconds after the readiness probe printed `ready!`. The pgvector image's entrypoint runs `initdb` against a *temporary* server and stops it before starting the real one; a single `pg_isready` over the unix socket is answered by that temporary server, so the probe could succeed inside the window and the next command would find the socket gone. A warm volume skips `initdb` entirely, which is why only a genuinely clean machine ever hit it. The probe now runs over TCP (the init server doesn't listen on TCP at all) and requires three consecutive successes, resetting if the server stops answering.
- **A clean tree no longer aborts the release** — the private-data guard read `git grep`'s status with a bare `out=$(…); rc=$?`. The release script runs under `set -Eeuo pipefail` with an ERR trap that exits, and `-E` propagates that trap into the command substitution — so rc=1, which is the guard's *clean* "no matches" case, killed the subshell before the status was ever classified, and a spotless tree surfaced as "the denylist could not load". The guard was inverted: the cleaner the tree, the more reliably the release aborted. The status is now read in a condition context, where neither errexit nor the trap sees it. The existing suite ran under plain `set -u`, which is exactly why it stayed green while the release failed; new cases re-run the guard in a child shell wearing the release script's own flags, and pin that the trap must keep reaching subshells (bash does not fire a parent's ERR trap for a failing `( … )` block, and every build step is one).
- **Private data no longer leaks into the system prompt** — browser-login domains and the home directory are kept out of the chat system prompt, home-rooted paths are abbreviated in LLM-visible tool output and in coding-agent folder-resolution errors, and the release guard's private-data denylist moved out of tracked source (fails closed on an unterminated block or a grep error, and keeps git's stderr out of the hit list). The tracked heuristic is now shape-only and names nobody: contributor names moved into a separate exceptions list that denies each name outright and enumerates its legitimate attribution sites — strictly stronger than the old pattern, which had been walking straight past a bare personal GitHub org in CI config.
- **Compose drafts** — a draft whose text was already submitted is dropped rather than resurrected; an answer never consumes an image-bearing draft; a draft the server still holds is never superseded; only a server report proves what the server holds; the pending flag is held until the last write settles.
- **Agent vision** — explicitly-requested and injected images stay in the model's vision instead of being stripped by the context trimmer, with a bound on how many stay pinned.
- **Background-task drain internals** — closes the lost-wakeup window in the `bash_output` injection wake, makes the finish wake durable for every concurrent waiter, reserves the pending-injection count before sending, scopes the injection-drain decrement to its own registration, reports the real wait when a task is evicted mid-drain, and accounts for buffer-cap loss in the truncation marker.
- **Version status** — an older on-disk binary (engine *or* gateway) is no longer read as a *new* version; an abbreviated same-commit id counts as not-older; the self-heal give-up is announced once instead of every tick; the frontend announces a new version only when one exists.
- **Changes / Apply** — the engine rebuild starts on every apply path, never fires for a non-Lucidos-source change, and the post-apply refresh runs only for the accepted `ChangeApplied` (not a suppressed duplicate), with the decision logged.
- **Thread type survives a continuation** — `ContinuationStarted` no longer relabels a chat thread as a coding-agent thread, threads already flipped are repaired on boot, and the channel gate is documented. A resumed *trigger* thread now continues on its own channel instead of having its source rewritten to `chat`, resolved through a shared channel decoder that also accepts the legacy alias.
- **Question-parked threads survive restart** — preserved across every abort path, resumed on their originating channel when answered, with no stale restart reminder posted on resume.
- **Question dividers** — an answered question divider re-anchors below a child-completion card, and only an *unresolved* divider is exempted from the child-completion redirect.
- **Orphaned Thinking marker** — a Thinking marker left stranded when a child-thread completion takes the turn is now dropped, with the Thinking-only finalize folded into the shared pending-step resolution rather than living as a special case.
- **Notifications** — the app-icon badge is re-asserted rather than diffed; the badge and unread list are single-sourced; the unread set reconciles after a cold-start mark-read; toasts stop gluing the notification title onto a structured body's first line.
- **Credentialed proxy** — `Sec-Fetch-Site` is authoritative in the guard (superseding the reverted `x-forwarded-host` reconstruction), and the gateway strips inbound `x-forwarded-host`.
- **Email** — network phases are bounded (IMAP 120s ops, SMTP 80s send under a 120s client budget) so a send surfaces the real error instead of a 10s client timeout; duplicate sends are guarded; the confirm modal remounts when a different draft replaces the open form.
- **Stop & queued messages** — Stop returns queued messages to compose instead of re-running them, awaits in-flight queued-message removal (trash-then-Stop race), and excludes mid-trash messages from the queue clear.
- **Mobile / iOS** — welcome suggestions stay tappable while composing, the wider header reserve is scoped to the brand row, `viewportIsMobile` self-corrects so an iOS PWA can't strand the desktop layout, and a stale entry bundle no longer wedges boot.
- **Coding-agent recovery** — conflict resolution survives a stray-kill auto-recovery with a session-branch-keyed hand-off; an app coding-agent resume reuses its surviving branch instead of failing on `-b`; the restart auto-resume race is closed by subscribing before backfill, with an orphaned-continuation startup safety net.
- **Boot** — the embedding model loads in the background and never blocks boot, with the memory rebuild and extraction guarded against the empty-embedder window.
- **Todo panel** — strikethrough applies to the item text rather than the row chrome, and abandoned items aren't struck through at all.
- **SDK bundle** resolves from the checkout instead of a fixed hop count above the binary.
- **LLM serialization** — tool results are ordered before user text in the OpenAI-compatible serializer; Gemini grounding is pinned to the global endpoint.
- **Intent sub-loops** surface their narrative text to the parent thread.
- **Security & dependencies** — `serde_with` 3.17.0 → 3.21.0 (GHSA-7gcf-g7xr-8hxj) and an npm audit fix for the postcss path-traversal advisory (GHSA-r28c-9q8g-f849).
- **CI & tests** — repaired the docs strict build and the Linux/aarch64-darwin release-tarball jobs; guaranteed the gitignored `VERSION` on all `build.rs` paths; the e2e database is rebuilt from zero on every run; 19 clippy lints cleared so the warnings-as-errors gate passes; build-lock tests no longer flake on a forked child's inherited fd; the channel decoder is pinned to its serde representation; every project's failure artifacts are kept for the whole browser run; the e2e worktree prune stays reachable on every cleanup path and the pre-kill hook reports its real exit code. The release scripts gained 66 new offline assertions covering the notarize resume handle and the credential-resolution path, plus 45 more for the compiled-input fingerprint gate, the re-fold reuse decision, and the staple-time DMG hash pin.

### Removed
- The card-less chat-redesign sandbox page (`chat-redesign.html`).
- Dead `--initiator-*` theme tokens.
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
