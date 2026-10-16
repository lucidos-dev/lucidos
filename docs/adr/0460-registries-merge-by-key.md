# 0460: Registries merge through keyed git merge drivers, not union; amends 0040

- **Status**: Accepted
- **Date**: 2026-10-12

## Context

Parallel coding-agent threads kept colliding on the same few files. Over 21
days the dev workspace saw 85 conflicts at Apply, about 74 hours in all. It
also saw 168 failed merges of `main` inside `/harden`.

Seven registries drew a large share: the ADR index, `docs/code-review-priors.md`,
`docs/temporary-measures.md`, `CHANGELOG.md`, both glossaries and
`.claude/rules/db.md`. Agents add entries at the top of a section, so every
concurrent addition shares one anchor. Two different entries are no conflict in
meaning, but a line merge stops on them.

The ADR index had `merge=union` (ADR 0040). It never conflicted, but union is
textual. It writes our lines and then theirs, so two appends come out of order.
When one side has already re-sorted, it keeps a line twice. `/harden` then
failed `check-adrs.sh`, and the agent committed a repair. That happened 91
times in 21 days, and each repair became the next union hunk.

## Decision

Paths named in `.gitattributes` merge through `lucidos merge-driver`, a hidden
subcommand of the `lucidos` CLI. `scripts/merge-drivers.sh` registers it in the
repository's shared git config, and `/harden` runs that before merging `main`.

- `lucidos-sorted-lines` (the ADR index): entries keyed by number, kept sorted,
  one per number.
- `lucidos-blocks` (the other six registries): markdown entries keyed by
  heading path, or by a bullet's bold label or first line. Different entries
  added at one spot both stay. An entry both sides edited gets git's line
  merge, so only a real same-line edit conflicts.

Union merge is retired. Generated files keep merging as plain text: a
`lucidos-generated` driver was built and dropped, see Alternatives.

## Rationale

**A driver runs in every merge, and only one definition exists.** Git calls it
for the agent's own `git merge main`, a resolver's merge, the engine's Apply
catch-up and the `merge-tree` probe behind the pending-change conflict flag.
Engine-only resolution would have missed the agent-side merges, 168 of 253.

**It refuses rather than guesses.** These cases stay conflicts:

- the same entry edited on the same lines on both sides;
- an entry deleted on one side and edited on the other;
- both sides reordering entries differently.

The driver hands a file back to git's merge in three cases:

- two entries share one key;
- the merge would put an entry under another heading;
- an entry is gone from both sides while either side added one. A key is the
  first line, so that may be a double rename or a rename against a delete. It
  may also be a closing `---` each side carried past its own new entry.

A driver that cannot run exits outside 0 or 1: a missing binary, an old CLI, a
crash. The registered command then merges the path without it: as plain text,
or by union for the ADR index. So a broken driver never leaves an unmarked
"ours".

**History confirmed it.** `scripts/replay-merge-drivers.sh` replayed 1,247
merges of `main` into agent branches, against the merges they had (union on
the index):

- On the six registries, 14 conflicted files merged by entry. 10 merges became
  fully clean, and every resolution checked matched or beat the agent's. One
  agent's hand resolution had dropped a priors entry its own branch added.
- On the ADR index, union never conflicted, so the gain is order. Against a
  text merge, the driver resolved 195 index conflicts, sorted and one per
  number. The 16 that differed from what was committed were union's doubled
  lines.

## Consequences

- Concurrent additions to the seven registries stop conflicting once the
  drivers are registered. The ADR index stays sorted without repair commits.
- A clone without the registration merges the ADR index as plain text, where
  union never conflicted. Every agent session registers in `/harden`, so this
  reaches only a fresh clone or a merge before the first `/harden`.
- `check-adrs.sh` keeps its order and duplicate checks. They cover a clone
  without the drivers, and hand edits.
- The scope is this repository. The CLI already holds the code, so offering the
  drivers to users later is registration by the engine plus docs. The engine
  would never write a user's `.gitattributes`.

## Alternatives considered

- **Keep union, add more paths to it.** Rejected: union reorders and doubles
  lines, as the 91 repair commits show. On prose it duplicates reworded text.
- **One file per entry, assembled by a generator.** Rejected: it changes every
  reader, including the runtime glossary load, links and grep habits. The
  assembled file would itself be a new generated file to merge.
- **A `lucidos-generated` driver** (one side's version, or a conflict when both
  changed it). Built and dropped. The replay showed it turns 61 clean text
  merges into conflicts, and only about one in six of those needed a regenerate
  afterwards. The root cause is committing generated output at all, which a
  separate plan takes on.
- **Engine-side resolution inside the Apply merge.** Rejected: it covers Apply
  only, and duplicates what git's driver hook already gives every merge path.
- **ADR numbers allocated at Apply.** Rejected: one renumber in 21 days, and
  rewriting agent content after hardening breaks ADR 0345's intent.
- **`git rerere`.** Rejected: a shared cache of hunk resolutions cannot
  generalise to new entries, and it can replay a wrong one silently.
