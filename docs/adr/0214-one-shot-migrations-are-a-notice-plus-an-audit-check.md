# 0214: A one-shot migration is a release notice plus an audit check

- **Status**: Accepted
- **Date**: 2026-09-18

## Context

The notification `tap` field changed from a four-string union to a
discriminated union object. Old workspaces still had triggers and apps posting
the strings, and the new engine rejects those with 400 at write time. So the
change shipped with `system-knowhow/migrate-tap-shape.md`, a recipe describing
how to rewrite them.

That file was the only entry in `system-knowhow/` whose job was to expire. Every
other file there documents a surface that exists today. This one documented a
shape that no longer exists, and it was meant to run once per workspace.

The layer charges rent for the privilege. `build_system_knowhow_section` splices
every file's `name` and `description` into the System Knowhow routing list. That
list enters the system prompt of every turn of every thread in every workspace.
The entry cost 302 chars of a 9,428-char list, about 3%, billed from May until
it was removed. It is one of the metered lines in
`always_loaded_context_stays_under_budget`, and a commit in August had just
finished trimming the same list.

Nothing recorded that it should ever go away. There was no row in
`docs/temporary-measures.md`, which is the registry for exactly that. So the
default outcome was permanence.

## Decision

A one-shot migration ships as two things that already exist, and never as a new
routable system knowhow:

1. **A release notice** in `release-notices.toml`, when the break needs a nudge.
   It is version-gated by `since`, cursor-tracked per workspace, and its one
   button sends a prompt.
2. **A check in `system-knowhow/workspace-audit.md`**, which owns detection, and
   whose § Remediation already owns fixing.

The detection patterns live in the audit check. The old-to-new mapping lives in
the audit's § Remediation. Neither gets a file of its own.

## Rationale

The audit already has the whole machine. § Remediation asks once, spawns one
child thread per target, and folds outcomes back into the same report. A
migration recipe beside it reimplements a parallel track: its own traversal
table, its own report format, its own completion event, its own idempotency
note. Every one of those is a second, slightly different copy of something the
audit does for every other finding.

Release notices are the mechanism for "do this once after upgrading", and the
tap break never got one. Both notices that do exist push to "Audit my workspace
for drift." So the migration bought a permanent per-turn cost and no one-time
reach to the users who needed it. That is the wrong way round on both axes.

The split was also inverted. The recipe owned the detection patterns and the
audit cited them, so the routine sweep depended on the one-shot file. The
remedy sat behind a file boundary from the finding it remedied, for one check
out of dozens. That ownership was deliberate, set by
`docs/plans/2026-08-25-system-knowhow-freshness-and-audit-dedup.md` on the
reasoning that the recipe is the file which rewrites. Deleting the recipe
retires the premise: the audit rewrites now, so the audit owns the patterns.

And it did not scale. Tap shape is not special. By the same logic every future
shape change earns a `migrate-<x>.md`. The routing list then fills with recipes
for migrations every live workspace finished long ago, and nothing removes them.

## Consequences

- The routing list keeps one entry per live surface, and nothing else. A file
  there is a claim that the surface exists now.
- Detection and remedy for a drift finding sit together, in the audit.
- A migration no audit check can express is a signal to re-examine the break. It
  is not a licence for a new routable file.
- The audit grows over time. That is the right place for it to grow. It is
  loaded on demand, not billed per turn, and a check whose drift is extinct can
  be deleted on its own.
- Writing rewrite guidance into the audit does not make the sweep an editor. The
  sweep stays read-only, and everything that edits sits under § Remediation.

## Alternatives considered

**Keep the recipe, but make it expire.** Add the missing release notice and a
`docs/temporary-measures.md` row with a removal condition. Rejected because the
condition is unwritable. It would have to be "no workspace has an old-form tap
left". That is a negative over private data on machines we cannot see, which
`.claude/rules/temporary-measures.md` rules out. The row would never resolve, so
the file would stay anyway.

**Keep it, but make it non-routable.** Park the recipe somewhere the routing
list does not reach. Rejected because no such place exists:
`SystemKnowhowStore::load_summaries` walks the shipped tree at unbounded depth
by design, so a subdirectory is still catalogued. Building an opt-out would add
a mechanism to carry one file.

**Write the rule and leave the tap recipe alone.** Record the pattern so the
next migration does not repeat it. Rejected because the example would then
contradict the rule, and the per-turn cost would keep being paid.

**Fold it into `js-sdk.md` beside the canonical `Tap` type.** That file already
owns the current shape. Rejected as the main home because a migration is a sweep
over a workspace's files, which is the audit's subject. `js-sdk.md` keeps one
sentence, for the app author who meets the 400 and needs to know the string form
is gone.
