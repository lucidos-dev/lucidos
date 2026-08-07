# ADR index

One line per decision, in number order. This file is **append-only** and
merges with `merge=union` (see `.gitattributes`), so two branches adding an
ADR at once both keep their line instead of conflicting. Union can leave the
new lines out of order: `./scripts/check-adrs.sh --fix` restores it.

Create an entry with `./scripts/adr-new.sh`, never by hand. See
[README.md](README.md) for the format and what belongs here.

- [0001: External-repo coding-agent thread surfacing: keep the carve-out](0001-external-repo-thread-surfacing.md)
- [0002: Lucidos Agent command safety: gate the dangerous slice, not every command](0002-lucidos-agent-command-safety.md)
- [0003: Agent-session loop and chat agentic loop stay separate; no shared loop orchestrator](0003-twin-agent-loops-stay-separate.md)
- [0004: Codex integrates as per-turn `codex exec` processes; backend locked per thread; coding-agent channel names stay `claude_code`](0004-codex-as-second-coding-agent.md)
- [0005: Codex defaults to the `codex app-server` protocol; `codex exec` stays as the escape hatch](0005-codex-app-server-protocol.md)
- [0006: Compose destination picker: one "To:" pick, remembered coding-agent chip, no auto-routing](0006-compose-destination-picker.md)
- [0007: One Thread Queue gates all background spawns; user chat preempts; restart re-fires, never replays](0007-thread-queue-admission-control.md) *(the "user preempts and doesn't count" part superseded by 0008)*
- [0008: User-initiated work shares the one capacity pool: prioritized, but counted and queued (reserves a background floor)](0008-user-initiated-work-counts-against-the-pool.md)
- [0009: An empty completion is an error only when it's genuinely an error, classified per-cause across providers](0009-empty-completion-classification.md)
- [0010: The user-half of the Thread Queue pool mirrors `thread_summaries.status` (one reconcile point, not hand-synced acquire/release)](0010-user-pool-mirrors-thread-status.md)
- [0011: A blocking child's completion is durably delivered to its parent: the persisted `ChildThreadCompleted` is the source of truth, the in-memory wake is a cache rebuilt on boot](0011-parent-child-fan-in-durability.md)
- [0012: Self-contained desktop app: the launcher owns Postgres + engine, the engine serves the UI same-origin, first-run falls back to mock, auto-update ships via GitHub Releases](0012-self-contained-desktop-app.md)
- [0013: Multi-workspace via a workspace gateway: one reverse-proxy fronts N always-on per-workspace engine stacks, addressed by path prefix](0013-multi-workspace-gateway.md) *(refined by 0014: standalone gateway crate, `/<workspace>` prefix, shared Postgres cluster, engine-served frontend)*
- [0014: Multi-workspace redesign: standalone gateway crate, `/<workspace>` path prefix, shared Postgres cluster, engine-served frontend](0014-multi-workspace-redesign.md)
- [0015: Restore lives in the workspace picker: local-file only, run via the engine `restore-archive` subcommand](0015-restore-in-the-workspace-picker.md)
- [0016: Packaged Tauri e2e is a boot smoke test, not UI automation (no WebDriver for macOS WKWebView)](0016-packaged-tauri-e2e-boot-smoke-test.md)
- [0017: `ask_user_question` caps options at 4, inherited from Claude Code's `AskUserQuestion` schema](0017-ask-user-question-four-option-cap.md)
- [0018: Agent surfaces (LLM tools, CLI, SDK) stay in sync via a capability parity manifest + codegen, not blanket parity](0018-capability-parity-manifest.md)
- [0019: Plugins get their own panel; plugin triggers auto-register with provenance](0019-plugins-panel-and-trigger-autoregistration.md)
- [0020: Deterministic builds via committed lockfiles, enforced with `--locked` / `npm ci`](0020-lockfile-determinism.md)
- [0021: The long-lived dev stack never runs from a coding-agent worktree](0021-long-lived-stack-never-runs-from-a-worktree.md)
- [0022: A workspace launches a *published* binary, one directory per build variant, never cargo's shared uplift path](0022-launch-binaries-are-published-per-build-variant.md)
- [0023: Web search is provider-native, resolved over the configured provider set (no keyless scraping, no dedicated search vendor)](0023-web-search-is-provider-native.md)
- [0024: The release candidate IS the published artifact: one stripped tree, built once, tested, then promoted](0024-the-release-candidate-is-the-published-artifact.md)
- [0025: The host-process kill guard is not defeatable by its own caller (ancestor-pid arm; a test file cannot signal what it didn't spawn)](0025-host-kill-guard-is-not-caller-defeatable.md)
- [0026: A coding-agent session is never owned by a request future, and a map entry always implies a live loop](0026-a-session-is-never-owned-by-a-request-future.md)
- [0027: A release does not wait on Apple: the DMG is deferred, labelled, and swapped in place](0027-a-release-does-not-wait-on-apple.md)
- [0028: The packaged window is a *remote* origin to Tauri's ACL, and is granted it explicitly](0028-the-packaged-window-is-a-remote-origin.md)
- [0029: A release tag names the main-line commit; the mirror's tag of the same name names the orphan (and the bump is *landed* on main, never skipped)](0029-a-release-tag-names-the-main-line-commit.md)
- [0030: `make lint` gates rustfmt, and the tree was swept once to make that possible (no `rustfmt.toml`; generated Rust emits itself formatted)](0030-rustfmt-gate-after-a-one-time-sweep.md)
- [0031: A deploy to a lucidos.dev origin runs on the maintainer's machine, never from public CI (the docs deploy workflow is deleted, not credentialed)](0031-deploys-run-on-the-maintainers-machine.md)
- [0032: A state write owns its announcement (the emit lives in the write path, and every announced surface is classified in one enforced registry)](0032-a-state-write-owns-its-announcement.md)
- [0033: The .app is notarized before the DMG, so a release waits on Apple once even when deferring (amends 0027)](0033-the-app-is-notarized-before-the-dmg.md)
- [0034: A CI-built artifact is ad-hoc by construction, so the headless front door stays unsigned (the Developer ID identity cannot leave the release machine)](0034-ci-built-artifacts-are-ad-hoc-by-construction.md)
- [0035: Worktree reclamation has exactly one owner, the cleanup worker, never a session teardown (the only session-path removal left is an explicit Discard)](0035-worktree-reclamation-has-exactly-one-owner.md)
- [0036: The release-candidate gate artifact is never public: a DRAFT release, fired by dispatch (drafts emit no `release` event)](0036-the-rc-gate-artifact-is-never-public.md)
- [0037: A down dependency is reported once by the layer that knows, never inferred N times downstream (one Docker probe, `database_reachable` on `/health`, one toast instead of twenty)](0037-database-reachability-is-reported-not-inferred.md)
- [0038: A chat link never leaves the workspace: the click handler's extractor chain is closed at the bottom, so an unclaimed href is a toast rather than an SPA-fallback reload](0038-a-chat-link-never-leaves-the-workspace.md)
- [0039: The public mirror's `main` is a linear release history, and a parent is safe because the mirror already publishes it (the orphan rule was sufficient, never necessary)](0039-the-public-mirror-is-a-linear-release-history.md)
- [0040: Concurrent ADRs stop conflicting: the index is a union-merged file of its own, and a number is allocated across every unmerged branch rather than read off main](0040-adr-index-is-union-merged-and-numbers-are-allocated.md)
- [0041: Coding-agent branches are named for their thread, not a timestamp](0041-coding-agent-branches-named-for-their-thread.md)
- [0042: A GA release is a draft until every platform tarball is attached](0042-release-is-complete-when-it-goes-public.md)
- [0043: The parent-to-child edge is the only privileged cross-thread write, and it asserts nothing](0043-parent-to-child-privileged-write.md)
- [0044: SDK-rendered surfaces render themselves in a hostless app window; the popout stays a bare app tab](0044-sdk-surfaces-render-without-a-host.md)
- [0045: The engine, not a client timer, decides when a switch-interrupted thread gets its Continue button back](0045-switch-resume-promise-discharged-by-engine.md)
- [0046: Freeze a superseded plan and let the glossary carry current truth](0046-freeze-superseded-plans.md)
- [0047: A thread's event wait is an event; the dispatcher is a cache rebuilt at boot](0047-event-wait-is-an-event.md)
- [0048: The deep-link mechanism is an ordinary HTTPS URL into the engine's own URL space; a `lucidos://` scheme is not it, and is not thereby rejected: one scheme cannot name one instance out of several, but an OS-level handoff stays open](0048-deep-links-are-https-into-the-engines-url-space.md)
- [0049: Every event wait is detached: a subscription never holds a turn](0049-every-event-wait-is-detached.md)
- [0050: The loopback API is unauthenticated, so attribution is evidence-based: an unattributed caller cannot claim to be the user](0050-unattributed-caller-cannot-claim-to-be-the-user.md)
- [0051: Chat file tools reach .lucidos/tmp/ read-only, never write outside data/](0051-file-tools-scratch-boundary.md)
- [0052: A thread subscription outlives Stop, and every other end of one is announced](0052-thread-subscription-outlives-stop.md)
- [0054: The packaged client enables macOS private API so the app view's Fullscreen is real fullscreen](0054-macos-private-api-for-element-fullscreen.md)
- [0053: A time window over an event-store column is resolved by the database, never by the caller's clock](0053-event-time-windows-are-resolved-by-the-database.md)
