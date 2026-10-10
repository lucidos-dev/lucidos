# 0115: Knowhow listing depth, a doc is at most one group folder deep

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

`collect_md_files` walked a knowhow root without a bound, so every `.md` at any
depth became a listed, routable id. The Know-how routing list is billed on every
turn of every thread, so each file cost the user forever.

That made one shape impossible: a doc with its own supporting material. A long
endpoint table is worth keeping and worth loading on demand. It is not worth a
row in every thread. Split out, each part took a row of its own, and the list
read as a pile of fragments.

## Decision

A knowhow **doc** is a top-level file in its namespace root, or a top-level file
in one group folder inside that root. Anything deeper is a **reference** that
belongs to a doc.

| Root | Listed | Deeper |
|---|---|---|
| `data/knowhow/`, shared `~/.lucidos/knowhow/` | `<name>.md`, `<group>/<name>.md` | reference |
| `data/apps/<id>/knowhow/` | `<name>.md` | reference |
| `data/triggers/<slug>/knowhow/` | `<name>.md` | reference |

Only listing changes. Resolution is untouched: a reference keeps its full id and
`load_knowhow` reads it exactly as before.

## Rationale

The two caps differ because the roots differ. An app or a trigger is already the
group, so a folder inside its knowhow dir can only be one doc's material. The
top-level root has no such owner, so its group folder is where a domain's docs
live, and six live docs sit there today.

The cap is an explicit `KnowhowListDepth` argument rather than a boolean or a
constant. The two shapes take different values, so the call site states which
rule it is applying, and a new root has to answer the question.

Leaving resolution alone is what makes the change safe to ship. Nothing that
resolved before stops resolving, so no id in an intent's frontmatter, a doc
body, or a user's habit can break.

## Consequences

- A doc can carry references in a folder named after it. They cost nothing per
  thread, and the doc names their ids so a reader can load them.
- A file placed too deep goes quiet: loadable, but in no routing list, and
  nothing fails. `system-knowhow/workspace-audit.md` § 3 flags a reference no
  doc names, so an audit is how the user learns a reorg is needed.
- `/api/v1/knowhow` and `lucidos knowhow list` show docs, matching the routing
  list. One definition of "a doc" serves every surface.
- Engine-shipped `system-knowhow/` still lists every file at any depth. That
  corpus is curated in the repo, where a stray depth is a bug we fix at source.
- `load_app_knowhow` still recurses. It dumps bodies into an executing app
  intent instead of building a listing, so a reference folder under an app's
  knowhow reaches that prompt whole.

## Alternatives considered

**One cap for every root.** Depth 1 everywhere loses the six `lucidos-ops/*` and
peer docs that live in a group folder. Depth 2 everywhere makes
`apps/<id>/knowhow/refs/` routable, which is the shape we set out to allow.

**A boolean, or a private constant per root.** Cheaper to write. It hides which
rule a call site is applying, and a new root then inherits whatever the flag
happened to mean.

**Cap resolution too, so a deep file is unreachable.** Tidier as an ontology,
and it breaks every existing deep id at once for no gain. Listing is what costs
tokens; resolution costs nothing until someone asks for it.

**Filter the listing on a frontmatter flag instead of on depth.** It puts the
decision in the file rather than in its placement, so a doc and a reference look
identical in the tree. It also needs a migration, since every existing file
lacks the flag.
