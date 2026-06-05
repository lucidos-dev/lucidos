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
