# 0006 — Compose destination picker: one "To:" pick, remembered coding-agent chip, no auto-routing

- **Status:** Accepted
- **Date:** 2026-06-12

## Context

The compose view's picker row was a `[Lucidos | Claude]` segmented control
that, on "Claude", revealed a breadcrumb of two more dropdowns:
`› [scope] › [Claude Code | Codex]`. Two structural problems (an internal
work-tracker item, plus direct user feedback "Selecting
claude then repo then claude or codex — that was NOT great"):

- **Wrong names.** "Claude" labeled the coding-agent *channel* (which can run
  Codex), and "Lucidos" appeared twice with different meanings — the mode chip
  (Lucidos Agent) and the scope dropdown's default (the Lucidos source repo).
- **No consequence-surfacing.** Nothing said the Lucidos Agent can hand off to
  a coding agent, that a coding target produces a reviewable *change*, or that
  an external target needs a registered *repository*.

Design dialogue 2026-06-12; plan in
`docs/plans/2026-06-12-compose-destination-picker.md`.

## Decision

1. **One destination picker.** A single "To:" dropdown (*compose destination*,
   now a glossary term) merges the actor + target decision: Lucidos Agent on
   top (default), then coding targets grouped under "Coding agent on…"
   (Lucidos source / apps / repositories), plus a "Register a repository…"
   action row. A dynamic caption under the picker states the consequence of
   the current pick.
2. **Coding-agent chip, remembered per workspace.** The Claude Code vs Codex
   pick is a secondary chip shown only for coding targets, persisted as the
   `coding_agent_default` preference. This *reverses* the earlier
   session-only default on `selectedCodingAgent` ("Claude Code is the safe
   default each session") — that comment had no ADR/test behind it, and the
   sibling decision on this exact surface (sticky `inputMode`, commit
   `5b5e52c30`) had already settled stickiness as what the user wants.
3. **No auto-routing.** No "Auto" destination and no prompt-classification
   hints this iteration. The guidance copy makes "start with the Lucidos
   Agent — it can hand off to a coding agent" the default path for unsure
   users; that *is* agent-driven routing, using the agent we already trust.

## Rationale

The choice the user actually makes is "where is this work going", not "which
actor implementation do I want". One picker makes that a single decision with
the consequences written next to it; the old chain forced three decisions
whose vocabulary contradicted the glossary ("Claude" ≠ Claude-Code-only,
"Lucidos" ≠ one thing).

## Consequences

- UI-only: wire format (`mode`, `use_claude_code`, `folder`, `coding_agent`),
  the `claude_code` channel value, and `sendCompose`'s bind-at-promotion logic
  are untouched (ADR 0004's naming carve-out stands).
- The presentation union (`ComposeDestination`) spans two storage shapes:
  per-draft synced `draft.mode` and device-global `selectedScope`. That
  asymmetry is pre-existing and kept; moving scope into the draft is a
  possible follow-up.
- The hand-off hint retires itself (preference
  `compose_handoff_hint_dismissed`) on the first coding-destination send or
  explicit dismiss.
- Persisting the chip adopts the established optimistic-write + refetch
  preference pattern (same as the chat model pick): a `loadPreferences`
  refetch racing an in-flight `setCodingAgentDefault` PUT can briefly revert
  the chip to the stale server value. The window is sub-second and the chip
  and the send-time binding read the same signal, so the UI never lies about
  what a send would do — accepted, not special-cased.

## Alternatives considered

- **Sharpened segmented control** (rename chips, merge dropdowns, keep
  two-level structure): keeps both top-level options always visible, but
  keeps the two-step mental model the user specifically disliked. Lost.
- **No picker — Lucidos Agent routes everything**: boldest simplification;
  adds an LLM hop + routing-trust requirement for direct coding tasks, and
  hand-off spawns sub-threads rather than top-threads. Deferred, not refused —
  revisit once hand-off UX is proven.
- **"Auto" destination / smart "looks like a code change" hint**: per the
  work item itself, "later, once we trust the heuristic". A misfiring
  heuristic erodes trust faster than a missing one builds it. Deferred.
