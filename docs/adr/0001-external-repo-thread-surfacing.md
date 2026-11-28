# 0001 — External-repo coding-agent thread surfacing: keep the carve-out

- **Status:** Accepted
- **Date:** 2026-06-05

## Context

An *external-repo coding-agent thread* runs Claude Code against a user-registered
*repository* (a separate git repo, not the Lucidos workspace). When it idles
with committed work, `thread_summaries.coding_agent_has_diff` is set to git-truth
by the `CodingAgentIdled` projection arm.

These threads are deliberately **excluded** from the proposal/review machinery
that internal coding-agent threads use (the documented carve-out in
`is_blocking` / `is_attention_needing` and `agent_recovery/has_diff.rs`): they
never set `coding_agent_proposed`, never get a `changes` row, never show an
Apply button, and resolve via **Archive**. The rationale already in the code:
*"engine doesn't own proposals for external repos (CC pushes/PRs from the
session)."*

The request was to make an external thread's diff more discoverable — concretely,
to show the drawer **"changes" dot** (`resolveVisualStatus` → `'changes'`, today
gated on `coding_agent_proposed`) when such a thread has a diff. The grill then
expanded the question to: should the diff also bump the thread to the top of
**Review** (tier-0), block the parent from archiving (ancestor-block), force an
explicit Discard, or get a Discard→revert action?

## Decision

**Keep the current non-involvement. Make no change.** External-repo
coding-agent threads stay out of the dot / tier-0 / block / confirm / revert
machinery. The diff remains discoverable via the in-thread **Diff** button
(already shown by `WaitingBanner` whenever `coding_agent_has_diff` is true), and
the thread remains freely archivable.

## Rationale

The whole proposal collapses onto one fact: **`coding_agent_has_diff` is
lifecycle-blind.** It means only "this branch differs from its base." It cannot
distinguish *unpushed* from *pushed* from *merged*, and it is never recomputed
for a finished thread — so for a normal feature-branch → PR → merge flow it stays
`true` for the thread's entire life, including long after the PR lands.

Every layer we considered building on that signal therefore **over-claims, and
the over-claim is permanent:**

- **"changes" dot** — reuses a dot whose established meaning is "actionable
  pending changes to Apply." For an external thread it would mean "has a
  (possibly already-merged) diff," and it would stay lit forever. Semantic
  overload + permanence.
- **tier-0 sort** — claims "needs attention now" → pins the thread to the top of
  Review permanently.
- **ancestor-block** (`is_blocking` / `is_attention_needing`) — claims
  "unresolved work" → the parent can't be cascade-archived and is pinned to
  Review until the child is individually archived, even weeks after the child's
  PR merged. A permanent wedge.
- **confirm-on-Archive guard** — claims "unmerged, at risk" → fires on *every*
  external-with-diff Archive, through the entire safe pushed-PR period, and its
  copy ("haven't been merged") becomes a lie post-merge. It cries wolf, so users
  learn to reflexively dismiss it and it protects nothing.

The only signal that would *not* over-claim is "branch is merged into remote
main" — but it is the **least reliably detectable** of all: squash/rebase merges
(GitHub's default) defeat both ancestor and patch-id checks, a moving main
defeats tree-diff, and it would require a network `git fetch` + repo auth inside
the `CodingAgentIdled` projection, a path that is local today.

So the existing carve-out is not an oversight — it is the **correct response to
not having a trustworthy signal.** The agent already self-manages push/PR from
within its session, the Diff button already surfaces the diff inside the thread,
and Archive already dismisses (and clears `coding_agent_has_diff` via
`CLEAR_CODING_AGENT_FLAGS`). Adding machinery that asserts things we cannot back
is worse than the honest silence we have now.

## Consequences

- **Kept:** external diffs are inspectable via the in-thread Diff button;
  external threads stay freely archivable; no new code, events, migrations, or
  risk; the `is_blocking` / `is_attention_needing` invariant
  (`is_blocking = is_attention_needing OR Running`) stays intact.
- **Given up:** no at-a-glance drawer indicator that an external thread produced
  a diff — you have to open the thread to see the Diff button. Accepted as the
  cost of not over-claiming.
- **Reopen criteria:** if we ever gain a cheap, reliable "has this landed in the
  destination" signal (e.g. the agent self-reporting push/PR/merge state as an
  event at session end), revisit — the lifecycle-aware version of this feature
  becomes honest at that point.

## Alternatives considered

| Option | Why rejected |
|---|---|
| **Dot only** (reuse `'changes'`) | Overloads the dot's "actionable" meaning; stays lit post-merge (lifecycle-blind). |
| **Dot, distinct visual** | Avoids the overload but still permanent/lifecycle-blind, for marginal drawer-scannability value. |
| **Tier-0 sort** | Pins to top of Review permanently regardless of actual state. |
| **Ancestor-block on `has_diff`** | Permanently wedges the parent's archive until the child is manually archived, even after the PR merged. |
| **Soft block (attention-only)** | Same permanence; also breaks the `is_blocking = is_attention_needing OR Running` invariant. |
| **Force Discard before Archive** | Can't be backed by `has_diff` (never clears → Archive becomes unreachable, Discard-only, on already-merged work). |
| **Discard → revert action** | Destructive `git reset` on the *user's external repo* + a new `CodingAgentDiffReverted` event variant; large/risky for a convenience Archive already covers. |
| **Gate on "unpushed"** | Robust & local, but under-blocks — a pushed-but-unmerged PR wouldn't surface, contradicting the intent. |
| **Gate on "diff vs remote main"** | The only semantically correct signal, but unreliable (squash/rebase, moving main) and needs network fetch + auth in a local path. |
