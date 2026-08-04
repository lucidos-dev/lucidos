# Decision log (ADRs)

Short, append-only records of **decisions and the reasoning behind them** —
especially the ones where we deliberately chose *not* to build something, or
backed out of an approach after thinking it through. The point is so future-us
(and the next person who has the same idea) can read *why* instead of
re-litigating it from scratch.

This complements `docs/plans/` and `docs/notes/`:

- `docs/notes/`: thinking, before anything is decided. An architecture
  discussion written down so it is not lost. No commitment, may be abandoned.
- `docs/plans/`: how a feature is/was built (the design + the implementation
  steps). Reach for these when you're building something.
- `docs/adr/`: *why* a decision went the way it did, including roads not taken.
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

## Creating one

```bash
./scripts/adr-new.sh <short-slug> "<the one-line index text>"
```

**Never pick the number by hand.** Reading only `main` is how two branches end up
claiming the same one, which git merges cleanly because the filenames differ, so
the collision stays invisible until someone notices two ADRs share a number (it
has happened twice). The script allocates across `main`, every branch not yet
merged into it, and the working tree, then scaffolds the file and appends the
index line.

`./scripts/check-adrs.sh` enforces the result and runs in `/harden`: one index
line per file and one file per line, no duplicate numbers, index in order, and
the sections above present. `--fix` restores the order.

## Index

The one-line-per-decision index lives in [`index.md`](index.md), on its own so
it can be `merge=union` without that attribute reaching any prose. Two branches
adding an ADR at once therefore both keep their line instead of conflicting.
