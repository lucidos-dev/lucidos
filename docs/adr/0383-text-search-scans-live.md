# 0383: Text search scans the four data roots live, with no index, and its section sits after every name section in Search Everywhere.

- **Status**: Accepted
- **Date**: 2026-10-07

## Context

Search Everywhere found things by name only. The user wanted to type a phrase,
find it inside any workspace file, and open the file at the matching line.

Nothing served that over HTTP. The agent's `grep_files` reads files one at a
time, line by line. The `memory_entries` embeddings hold facts an LLM extracted
from `artifacts/`, not lines, so they cannot point at one.

The largest workspace measured holds about 3,700 text files and 55 MB of text
under `artifacts/`, `apps/`, `knowhow/` and `triggers/`. ripgrep searches that
in 0.10 to 0.14 s warm on all cores, 0.56 s on one core, and about 1 s cold.

## Decision

*Text search* is a Search Everywhere category. It scans the four browseable
data roots live on each debounced keystroke, with no index and no stored state.
It matches one literal phrase, ignoring case.

On the All tab its section always sits after every name section. Inside it,
lines rank by title match level on the line, then newest file, then path.

The scan lives in `engine/text_search.rs` and is served by
`GET /api/v1/search/text`.

## Rationale

The measurement makes a live scan fast enough. It must run in parallel, over
whole files, and stop early for the All tab's preview. A live scan is also
always fresh. An external edit, an agent's write and an app's own file all show
on the next keystroke. There is no event to miss and nothing to rebuild.

A name hit is stronger evidence than a phrase buried in a file. Letting text
lines compete under ADR 0348's ordering would also move sections between
keystrokes, as a common word's best line changes. Pinning Text last keeps the
name sections where the user expects them.

## Consequences

- Cost grows with the workspace's text. The first search after a cold start
  pays the disk reads, about 1 s on the measured workspace.
- Files over 2 MB are skipped. The Text tab always says how many, and the
  All tab says so when it found no line.
- A hit opens in source view, because only source view has lines.
- Registered repos are not searched. The agent's `grep_files` still reaches
  them.

## Alternatives considered

- **A separate find-in-files panel (Mod+Shift+F).** Better for wide sweeps, with
  room for toggles, but a third search surface, and Mod+K would stay name-only.
- **A content mode in file search (Mod+P).** It only opens on the Files menu
  item and filters in the browser, so the content walk needs a new engine call
  anyway.
- **An in-memory text cache.** About 10 ms a search, but every workspace
  engine would hold its text in RAM and need an eviction rule, to save about
  0.1 s.
- **A Postgres trigram index over lines.** Fast at any size, and it survives a
  restart. But it adds a table, an indexing consumer and a backfill. It also
  needs a staleness path for edits that emit no event.
- **Registered repos as roots.** A large repo turns each keystroke into a full
  grep. Revisit with an index if repos are wanted.
- **Text competing by rank on the All tab.** Rejected for the section movement
  above.
