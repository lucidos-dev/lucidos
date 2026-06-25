# Decision log (ADRs)

Short, append-only records of **decisions and the reasoning behind them** —
especially the ones where we deliberately chose *not* to build something, or
backed out of an approach after thinking it through. The point is so future-us
(and the next person who has the same idea) can read *why* instead of
re-litigating it from scratch.

This complements `docs/plans/` and `docs/design/`:

- `docs/plans/`, `docs/design/` — how a feature is/was built (the design + the
  implementation steps). Reach for these when you're building something.
- `docs/adr/` — *why* a decision went the way it did, including roads not taken.
  Reach for this when you're about to revisit a settled question.

## Format

One file per decision: `NNNN-short-slug.md`, numbered in order. Each entry has:

- **Status** — Accepted / Superseded by NNNN / Reversed.
- **Date**.
- **Context** — what prompted the decision.
- **Decision** — what we chose, in one or two sentences.
- **Rationale** — why. This is the part that matters.
- **Consequences** — what follows from it (what we keep, what we give up).
- **Alternatives considered** — each option weighed and why it lost. A rejected
  option with its reason is worth more than the chosen one alone.

Keep entries scannable. A decision log nobody reads is just more drift.

## Index

- [0001 — External-repo coding-agent thread surfacing: keep the carve-out](0001-external-repo-thread-surfacing.md)
- [0002 — Lucidos Agent command safety: gate the dangerous slice, not every command](0002-lucidos-agent-command-safety.md)
- [0003 — Agent-session loop and chat agentic loop stay separate; no shared loop orchestrator](0003-twin-agent-loops-stay-separate.md)
- [0004 — Codex integrates as per-turn `codex exec` processes; backend locked per thread; coding-agent channel names stay `claude_code`](0004-codex-as-second-coding-agent.md)
- [0005 — Codex defaults to the `codex app-server` protocol; `codex exec` stays as the escape hatch](0005-codex-app-server-protocol.md)
- [0006 — Compose destination picker: one "To:" pick, remembered coding-agent chip, no auto-routing](0006-compose-destination-picker.md)
- [0007 — One Thread Queue gates all background spawns; user chat preempts; restart re-fires, never replays](0007-thread-queue-admission-control.md) *(the "user preempts and doesn't count" part superseded by 0008)*
- [0008 — User-initiated work shares the one capacity pool: prioritized, but counted and queued (reserves a background floor)](0008-user-initiated-work-counts-against-the-pool.md)
- [0009 — An empty completion is an error only when it's genuinely an error, classified per-cause across providers](0009-empty-completion-classification.md)
- [0010 — The user-half of the Thread Queue pool mirrors `thread_summaries.status` (one reconcile point, not hand-synced acquire/release)](0010-user-pool-mirrors-thread-status.md)
- [0011 — A blocking child's completion is durably delivered to its parent: the persisted `ChildThreadCompleted` is the source of truth, the in-memory wake is a cache rebuilt on boot](0011-parent-child-fan-in-durability.md)
- [0012 — Self-contained desktop app: the launcher owns Postgres + engine, the engine serves the UI same-origin, first-run falls back to mock, auto-update ships via GitHub Releases](0012-self-contained-desktop-app.md)
- [0013 — Multi-workspace via a workspace gateway: one reverse-proxy fronts N always-on per-workspace engine stacks, addressed by path prefix](0013-multi-workspace-gateway.md) *(refined by 0014: standalone gateway crate, `/<workspace>` prefix, shared Postgres cluster, engine-served frontend)*
- [0014 — Multi-workspace redesign: standalone gateway crate, `/<workspace>` path prefix, shared Postgres cluster, engine-served frontend](0014-multi-workspace-redesign.md)
- [0015 — Restore lives in the workspace picker: local-file only, run via the engine `restore-archive` subcommand](0015-restore-in-the-workspace-picker.md)
- [0016 — Packaged Tauri e2e is a boot smoke test, not UI automation (no WebDriver for macOS WKWebView)](0016-packaged-tauri-e2e-boot-smoke-test.md)
