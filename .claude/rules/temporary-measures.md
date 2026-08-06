# Temporary measures

**Always loaded** (no `paths:` frontmatter): the inclusion test has to be applied whenever a temporary measure is introduced, which is a judgment made before any particular file is touched.

A **temporary measure** is anything in the codebase that is *meant to end* and
carries a *concrete condition for when*: a workaround until upstream fixes X, a
diagnostic that only exists to chase a bug, a crutch for a recurring model
mistake, a feature flag awaiting cleanup, a back-compat shim with a real removal
trigger. Left untracked, each silently becomes permanent.

## The inclusion test

Apply one line to every candidate:

> **Is this meant to go away, and is there a concrete condition for when?**

**Yes** goes in the registry, [`docs/temporary-measures.md`](../../docs/temporary-measures.md).
**No** belongs elsewhere: permanent back-compat, a site-local `#[allow(...)]`, a
settled design decision (`docs/adr/`), or backlog with no end condition. If you
can't write a concrete removal condition, it isn't a temporary measure.

The registry owns the rest of the taxonomy: its four typed sections (workarounds,
model-tolerance measures, feature flags and sunset deprecations, open
investigations as parents), the full OUT list, and the field shape every entry
uses. Read it when you are about to add one. One judgment from it is worth
knowing before you get there: for a model-tolerance measure, **"the model" means
the weakest model in the model registry, not the newest**, so a new flagship is
never on its own evidence that a crutch is removable.

## What binds at write time

- **Log it in the same change.** A temporary measure that isn't logged silently
  becomes permanent, which is the entire failure this prevents. It also closes
  the loophole where rewording a `TODO: remove after X` into a plain comment
  dodged tracking: the escape valve is to **register it**, not to launder the
  marker.
- **Comment at the site**, pointing at the registry row, so a future reader knows
  it's removable and why.
- **State a concrete removal condition.** Exactly what has to be true to drop it,
  and how to verify that is safe. "Eventually" is not a condition.
- **Don't reach for a measure first.** Prefer making the right thing discoverable
  (docs, prompt, types). Only add a crutch when guidance has already failed, and
  say so in the entry.
- **Removing one updates the registry, it does not delete the row.** Flip Status
  to `removed` / `resolved` with the date and keep the row as history.

See also: `docs/glossary.md` § "temporary measure".
