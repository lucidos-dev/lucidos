---
globs:
  - "crates/**/*.rs"
  - "crates/lucidos-app/src/**/*.css"
  - "crates/lucidos-engine/src/api/*.css"
  - "crates/lucidos-app/src/**/*.ts"
  - "crates/lucidos-app/src/**/*.tsx"
  - "packages/lucidos-sdk/**/*.ts"
  - "system-knowhow/**/*.md"
  - "docs/temporary-measures.md"
---

# Temporary measures

A **temporary measure** is anything in the codebase that is *meant to end* and
carries a *concrete condition for when* — a workaround until upstream fixes X, a
diagnostic that only exists to chase a bug, a crutch for a recurring model mistake,
a feature flag awaiting cleanup, a back-compat shim with a real removal trigger.
Left untracked, each silently becomes permanent. They are tracked in
[`docs/temporary-measures.md`](../../docs/temporary-measures.md).

## The inclusion test

Apply one line to every candidate:

> **Is this meant to go away, and is there a concrete condition for when?**

**Yes → it goes in the registry** (`docs/temporary-measures.md`). **No → it belongs
elsewhere** (see OUT, below). If you can't write a concrete removal condition, it
isn't a temporary measure — it's either a permanent requirement or backlog.

## The four categories

The registry has four typed sections; a temporary measure is one of:

1. **Temporary measures & workarounds** — diagnostics, scaffolding, "workaround
   until upstream fixes X".
2. **Model-tolerance measures** — a crutch added *only* to compensate for current
   LLMs making a predictable, recurring mistake (a forgiving alias, a tolerated
   wrong name, a fallback that fires on a model error, a permissive parse for a
   shape the agent tends to emit). The honest reason it exists is "the model keeps
   getting it wrong," not "the design needs it." The test: *would we still want
   this if the model were perfect?* No → it's a tolerance measure. (This category
   absorbed the retired `model-tolerance-measures.md`.)
3. **Feature flags & sunset deprecations** — flags / kill-switches awaiting
   cleanup, and back-compat shims/aliases that carry a **concrete** removal
   condition (NOT permanent back-compat).
4. **Open investigations (parents)** — an investigation is the *reason* a measure
   exists, not a measure itself. Model it as a parent entry (no "Lives in" site);
   each measure references its parent investigation by id. Closing an investigation
   surfaces every measure now eligible for removal.

## Explicitly OUT (so the registry doesn't become a tech-debt swamp)

- **Permanent back-compat / old-data tolerance** (serde aliases for old events,
  parsers for legacy wire shapes) — real requirements that never leave.
- **Site-local suppressions** — `#[allow(...)]`, `@ts-expect-error`,
  `// eslint-disable`. Harden flags those at the site, not here.
- **Permanent design decisions / accepted non-bugs** — those live in `docs/adr/`
  and `docs/code-review-priors.md`.
- **General refactor wishlist / "tech debt" with no concrete end condition** —
  that's backlog / `docs/plans/`.

## Rules

- **Log it in the same change.** When you add a temporary measure, add an entry to
  `docs/temporary-measures.md` (in the right typed section, with every field —
  Added/Opened, Lives in, Impermanent because, Removal/resolution condition,
  Status, and for a measure the parent Investigation id) in the same commit. A
  temporary measure that isn't logged silently becomes permanent — that's the
  failure this prevents. This also closes the loophole where rewording a
  `TODO: remove after X` into a plain comment dodged tracking: the escape valve is
  to **register it**, not to launder the marker.
- **Comment at the site.** Leave a short comment where the measure lives saying
  it's a temporary measure and pointing at its registry row, so a future reader
  knows it's removable and why.
- **Every measure carries a concrete removal condition.** State exactly what has to
  be true to drop it and how to verify it's safe. "Eventually" is not a condition.
- **Model investigations as parents.** When a measure exists because of an open
  investigation, add (or reuse) the investigation parent entry and reference it by
  id from the measure. Don't bury the reason inside the measure's own row.
- **Removing one updates the registry, doesn't delete the row.** Flip Status to
  `removed` / `resolved`, add the date, keep the row as history. Also revert any
  paired docs/wording the measure softened (the entry's removal condition should
  name them). Closing an investigation flips every measure tagged with its id.
- **Don't reach for a measure first.** Prefer making the right thing discoverable
  (docs, prompt, types). Only add a crutch when guidance has already failed — and
  say so in the entry.

See also: `docs/glossary.md` § "temporary measure".
