# 0348: Search Everywhere ranks every category by one generic title rank, match level then coverage, and orders the All tab's sections by their best hit, rather than keeping a per-keyword map of the most relevant category.

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

Typing "settings" in Search Everywhere first listed an app that only mentions
settings in its description. A screen of vendored files came next. Only then
came the Settings page and the Open settings shortcut. Every hit scored the
same, the All tab listed sections in a fixed order, and each category cut at
its cap before it ranked anything.

The user asked whether a semantic layer should know, per keyword, which
category is most relevant.

## Decision

No per-keyword map. Every category ranks its hits by one generic *title rank*
before it cuts at the cap. The All tab orders its sections by the title rank of
each section's best hit, with the old fixed order as the tiebreak. The tab strip
itself never reorders, and each tab shows how many hits it holds.

The title rank is a match level (`None`, `Phrase`, `WordStart`, `Exact`) and
then coverage, the query's share of the title's length. One definition lives in
`engine/title_match.rs`. The frontend twin is pinned to it by a generated
fixture.

## Rationale

A keyword map is curated data about the product's own surface. It goes stale
with every new setting, page, app or file, and nothing tells you when it has.
The title rank needs no curation and already gets the motivating case right:
"Settings" is an exact title, "Open settings" a word start, and a description
hit ranks below both.

Coverage came from rendering the change against real data. Files such as
`settings-system-v2.png` are word starts too, so on match level alone Files
tied with Settings and the fixed order put Files first. Coverage separates them
without naming either category.

## Consequences

- A section can move up as its answer lands, since engine categories answer at
  different speeds. The keyboard selection follows its row, so Enter still opens
  the highlighted hit.
- Thread search also ranks a word-start title above a mid-word one. That is the
  same direction as its earlier exact-then-phrase rule.
- Files search leaves out vendored trees and build output through
  `is_build_output_path`. The agent's file tools still see them.
- A query that names a concept rather than a title, such as a synonym, still
  ranks by where the words fall. The Settings index's keywords keep finding
  such hits, but below a title hit.

## Alternatives considered

- **A per-keyword category map.** Rejected for the staleness above, and because
  it would answer only the queries someone thought to write down.
- **Embedding similarity for every category.** Threads already merge text and
  meaning. Embedding settings, pages, apps and file names would cost an
  embedding call per keystroke for labels a lexical rank already orders well.
- **Tab counts alone, keeping the fixed section order.** It shows where hits
  are, but leaves the best hit below a screen of weaker ones. The counts ship
  beside the ranking instead.
- **Exact totals per category.** Thread search's merge has no total. Asking each
  category for one hit past the All tab's cap tells "5" from "5+" for every
  category alike.
