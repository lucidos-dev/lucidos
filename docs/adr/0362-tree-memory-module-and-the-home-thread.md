# 0362: Memory becomes a module, Tree beside Classic, and one home thread reaches every thread

- **Status**: Accepted
- **Date**: 2026-10-04

Record: `docs/plans/2026-10-04-tree-memory-module-and-the-home-thread.md`.
Amends [ADR 0124](0124-a-new-thread-starts-clean.md) and
[ADR 0148](0148-voice-is-a-mode-of-a-thread.md). Keeps
[ADR 0168](0168-a-thread-acts-in-its-own-subtree.md) unchanged.

## Context

Long-term memory is lossy twice over. Inside a thread, everything past the
last 15 messages becomes one summary paragraph (ADR 0102). Across threads,
continuity rides on extracted, embedded facts (ADR 0124). Neither has a way
back to the exact words.

OptChat, published as a spec by the author of OptMem, shows a lossless shape.
Source: "OptChat - HOW TO REPLICATE MY SETUP" by Victor Taelin,
<https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449>,
gist revision f51fe5c9, read 2026-10-06. Section numbers refer to that
revision.
One append-only log carries a binary tree of summary lines. Each turn starts
fresh from a fixed-size view of that tree, so context never rots. A `zoom`
tool opens any line into its two halves, down to the raw message.

Lucidos already holds the hard part. Events are an append-only log, and a
thread is one view over them. What OptChat lacks is parallel work: it is one
linear chat, with subagents reporting into it.

Measured in the dev workspace over 7.5 months: 41,296 memory entries, 97%
derived from thread events and 3% from artifacts. Coding-agent threads get no
memory recall at all today.

## Decision

**Memory is a module, chosen per workspace.** *Classic* is today's history
summariser, memory recall and memory search. *Tree* is described below. Both
stay selectable. Tree becomes the default once the context benchmark (ADR
0110) scores it at least as well as Classic.

**Tree is a tree of trees, projected from events.**

1. Every *thread* has its own *summary tree* over its own events. Lucidos Agent
   threads log at full fidelity, tool calls and results included. Coding-agent
   threads log one prompt and one reply per turn.
2. The workspace has one summary tree. Its leaves are settled thread turns and
   artifact writes.
3. `zoom` runs straight through: workspace node, thread turn, thread tree,
   message. Zoom on an artifact leaf reads the file at that commit.
4. The events table is the log. Only summary nodes are new storage.

**A turn reads two memory views.** The *workspace memory view* and the *thread
memory view* are tilings of the two trees, sized in bytes and tunable per
surface. The default is 64 KB of workspace view for the home thread and 16 KB
elsewhere, plus a thread view of up to 64 KB.

**The workspace view is shared as a view snapshot.** The request order is:
tools, system prompt, view snapshot, thread memory view, recent workspace
entries, message. The recent block holds entries newer than the snapshot. The
snapshot rolls over when that block passes a size cap.

**Recall is four tools:** `zoom`, `find`, `search` and `date`. `find` walks the
tree with a *judgment provider*, batching rows the way pg-jev does. `search` is
text search over the log, returning tree addresses. ADR 0363 covers the
providers.

**One home thread per workspace.** It is a *chat thread* that never ends. It
heads the thread drawer, above every section rather than inside Pinned, and it
can be neither archived nor deleted. Its sub-threads show in the sections as
threads of their own. It reads and follows
up any thread, coding-agent threads included. It presses an owner button only
while the owner's words in that turn ask for it, exactly as ADR 0168 clause 5
already allows. The two widest Always-allow grants stay on screen only.

**Only the user names the home thread.** It is born "Home" and renamable by
hand. Nothing titles it automatically: no generated title, no suggested name.
A thread that never ends has no single topic, so a model naming it from one
exchange gets it wrong. The bus refuses `ThreadTitleGenerated` on it, the
title paths skip it before any model call, and its menu hides Suggest name.
Plan: `docs/plans/2026-10-05-home-thread-never-auto-titled.md`.

This supersedes ADR 0208. Calls run only on Home, and Home is never named
automatically, so call naming was retired from the engine.

**Voice sessions live only in the home thread.** Voice stays a mode of a
thread, and that thread is the home thread.

## Rationale

**Equal citizens, not a privileged chat.** Giving the home thread the only tree
would make every other thread a second-class memory source. The tree of trees
stores every thread at full fidelity and lets any thread reach any other.

**A flat interleaved log muddles its summaries.** Merges pair adjacent
entries, and adjacent entries from unrelated threads make a summary about
nothing. Thread trees keep each merge inside one conversation.

**The snapshot exists for the cache.** A cache prefix breaks at the first
changed byte. A live workspace view changes whenever any thread settles, and
it would take this thread's whole thread view down with it.

**A small preload is cheaper than a big one.** A deep lookup costs one `find`
call. A 48 KB larger preload costs about 24k tokens on every turn, and the dev
workspace runs about 340 trigger runs a day.

**The home thread needs no new authority.** ADR 0168 already lets a thread press
owner buttons on the owner's words in that turn. The home thread only widens
reach, never authority.

## Consequences

- Storage estimate for the dev workspace: about 275k tree entries, roughly
  170 MB, 300 MB at worst. `memory_entries` is 234 MB today.
- Backfill costs about 500k compactor calls once, then about 2,400 a day at
  that rate. Most user messages are under 512 bytes and stay verbatim.
- Coding-agent threads gain the workspace view at session start, and `zoom`,
  `find` and `search` through the `lucidos` CLI. Claude Code and Codex keep
  managing their own context.
- With a System One provider picked for `find`, tree summary lines go to that
  third party. Without one, a chat model answers instead.
- Voice is no longer offered on threads other than the home thread.
- The home thread ships off, behind the experimental `home_thread_enabled`
  switch. Off hides it and turns its powers off, voice included, and on
  brings the same thread back. Plan:
  `docs/plans/2026-10-04-home-thread-behind-an-experimental-toggle.md`.
- A trigger thread gets a summary tree once the owner writes in it. Its
  unattended runs before that stay out of the workspace tree, which the size
  and cost figures above already assumed.
- Tree positions stay a pure function of events, so deleting a thread moves
  the later workspace leaves up. Every workspace summary after the first
  deleted leaf is rebuilt, and the delete dialog names that cost.
- The compactor reads context from the node's own tree only. So after a
  delete, no surviving line was written from the deleted thread's words.

## Alternatives considered

- **Replace memory outright, in one change.** Lost: no measured evidence that
  Tree recalls as well. Modules also give a standing A/B.
- **One flat log across all threads.** Lost: muddled merges, see above.
- **The home thread logs alone, other threads post reports.** Lost: other
  threads become second-class in storage.
- **64 KB of workspace view everywhere.** Lost on cost, mostly trigger runs.
  The size stays tunable per surface.
- **A live workspace view, OptChat style.** Lost on cache misses.
- **Vector search over tree nodes.** Lost: about 850 MB of vectors, and a weaker
  judge than a judgment provider reading the lines.
- **The pg-jev extension inside Postgres.** Lost: it needs `plpython3u` and a
  superuser, so Python would ship inside the bundled Postgres.
- **A fresh coding-agent session every turn.** Deferred: it fights the coding
  agents' own caches and session state.
- **The home thread presses owner buttons on its own judgement.** Lost: it
  removes the owner from gated actions.

## Amendment (2026-10-05)

**I15's workspace view is off by default for a coding agent**, the
maintainer's decision. `workspace_view_bytes_coding_agent` now defaults to `0`, so a fresh
coding-agent session starts with no workspace memory view, same as Classic
today. A positive value still opts in, per workspace.

Two reasons. First, parity: Classic gives a coding agent no memory recall at
all (see Context above), so Tree should not turn one on by default either.
Second, a coding agent's context comes from the task text its spawner writes,
not from a reconstructed workspace history.

## Amendment, 2026-10-05: the view folds the spec's way, and its stable start is cached

Plan: `docs/plans/2026-10-05-tree-view-spec-fold-and-cache-marks.md`.

### The old fold was a §5.3 rebuild

The first fold rebuilt each view from scratch. It covered `[0, k)` with the
binary digits of `k`, showed every leaf from `k` on, and picked the smallest `k`
that fit. `k` moved every turn, so the first changed byte sat a few blocks into
the view. That is OptMem's `wake`, which OptChat §5.3 rejects because every turn
was a cache miss. It also fell off a cliff: in a 1,000-entry thread the oldest
750 entries showed as about seven lines.

### The new fold

The view follows OptChat §5.2. It appends each entry's line at its end. While
over budget, it merges the adjacent sibling pair with the largest `(T - start) /
2^(l+2)` whose parent is built. A merged line never splits. Each level then
holds about as many lines, and a level-`l` line changes about once every `2^l`
entries.

**We replay, and we replay only what is built.** Every read folds again from
entry 0, so a restart loses nothing. The replay covers the *built prefix*,
where every node is built. Those nodes never change, so a longer built prefix
replays to the shorter one's view plus appends and merges. Entries past it
follow as leaves, with their built line or the raw cut, in a reserve of
`min(8 KB, budget / 2)` that the prefix replay leaves free.

The reserve is the one deliberate departure from the spec. The spec keeps one
live view and waits for the compactor before each turn (§6). Lucidos does not
wait, so a replay that met an unbuilt merge would pick a lesser pair, and the
next turn's replay would undo it. In a simulation with the compactor one turn
behind, that cut the share at 10k entries and four-entry turns from 65% to 45%.
The prefix replay with a reserve brought it back to 56%.

The workspace snapshot uses the same fold, without a reserve, since it is
frozen per epoch. The compactor keeps the old tiling for its context, renamed
`context_tiling`, so its calls are unchanged.

### Measured prefix share

The share is the bytes two consecutive views have in common from the start,
over the newer view's size. Measured by
`consecutive_turns_share_most_of_the_view` on a synthetic, fully built log, a
64 KB view and 40 turns per row:

| Entries | Turn size | Old fold | Spec fold |
|---|---|---|---|
| 1,000 | 1 | 66% | 65% |
| 1,000 | 4 | 14% | 40% |
| 1,000 | 10 | 2% | 27% |
| 10,000 | 1 | 75% | 86% |
| 10,000 | 4 | 25% | 61% |
| 10,000 | 10 | 5% | 48% |
| 100,000 | 1 | 76% | 89% |
| 100,000 | 4 | 28% | 71% |
| 100,000 | 10 | 4% | 62% |

A turn merges up to level `log2(turn size)`, so the share grows with the number
of levels. A majority holds from 10k entries for turns of up to four entries,
and from 100k entries for turns of ten. A 1,000-entry view has four or five, and a ten-entry turn changes
most of it. The spec reports the same shape: 56% at 20k messages, 70% at 400k.

### Cache marks and the breakpoint budget

The thread view leads the message as its own blocks, cut at two marks. Each
mark is the last line end at or before 50/128 and 80/128 of the prefix
budget (`budget - reserve`), the spec's 50k and 80k of 128k. A mark past the view's end is skipped.
The spec's third mark, at 100/128, is dropped: in the simulation it never added
a read, and no request has a slot for it.

Anthropic allows four markers, and a fifth is a 400. The wire now spends them
in this order: the last message, the one before it, each memory view block
(the snapshot, then the thread marks), the system block, then tools.

- A Tree turn's first round marks the last message, the snapshot and both
  thread marks. Later rounds mark the last two messages, the snapshot and the
  first thread mark.
- A read needs no marker of its own. In a turn's first round, the lookback
  from the last message's marker reaches every view boundary in that message.
  So the next turn reads the longest piece still identical.
- The system block gives up its marker whenever both views are present. Every
  view prefix holds tools and system, so a system entry pays only when the
  snapshot misses: once per epoch per surface, when the snapshot rolls over.
- A turn with no snapshot gives the free marker back to the system block.
  Classic has no view block, so its request is byte for byte what it was.

### Settled: a turn never waits for the compactor

I7 settles this. A turn never waits for the compactor. An unbuilt leaf shows
its built children, or the raw entry cut to head and tail within 512
characters, with a note of how much was cut. This amendment does not change
that.

The spec waits instead (§6, checklist item 3). Lucidos does not, because
leaves build one at a time, in order: a turn of forty tool calls could leave
forty sequential compactor calls, minutes rather than seconds. A compactor
outage would then stall every Tree turn, not one line of it. The cut text
says how much was cut, and `zoom` opens the whole entry.

## Amendment, 2026-10-06: the compactor's default model

The compactor no longer inherits the memory model. Its default is resolved
against the configured providers from one ordered list, and it calls through
the router, so it runs without Vertex. ADR 0373 records the decision and
`docs/plans/2026-10-06-tree-compactor-provider-aware-default.md` the evidence.

## Amendment, 2026-10-06: Home leaves the thread drawer

The maintainer's decision. The drawer no longer draws the home thread above its
sections. It sits in no section either, so no thread list draws it. Two entries
open it instead:

- **Desktop**: a Home icon in the thread pane's header, beside New thread.
- **Phone**: Home, the first row of the Lucidos menu.

Both show only while the experimental switch is on. A thread that never ends is
a place to return to, not one more row to scan past. So it belongs with the
header's other ways in. The filtered drawer views (In flight, Needs attention,
Drafts) still list it when it qualifies, since they exist to surface such
threads. Its title menu drops Show in thread list, which has no row to show.

## Amendment, 2026-10-06: the backfill runs in hours, and ready comes early

The maintainer's request: the dev workspace's estimate read "1-3 days". Plan
and bench: `docs/plans/2026-10-06-tree-backfill-in-hours.md`.

### What held the backfill to days

- One global cap of 8 model calls, whatever the provider allowed.
- The workspace tree's leaves built one at a time, though none reads another
  workspace line. A turn leaf also waited on its thread's tree, so the
  workspace could not finish before every thread's leaves did.
- The ready flag waited on every tree, so turns stayed on Classic until the
  last old thread was summarized.

### The decisions

1. **Compactor lanes.** Each provider route allows 64 calls at once to start,
   between 1 and 128. AIMD: a success adds `1 / limit`, and a rate limit,
   overload or timeout halves it once per window. A live event's call takes a
   freed place first.
2. **A workspace turn leaf reads its turn's raw entries**, each cut to 512
   bytes as a view shows an unbuilt leaf. Its context is the same thread's
   earlier entries, cut the same way. It reads no summary line, so workspace
   leaves build at once and never wait on a thread. It still reads only its
   own thread, so a delete leaves no line written from the deleted words.
3. **A build is a function of the log and the answers.** A drain applies
   finished nodes in launch order. So a tree is the same whichever order its
   scope was queued, and however long each call took.
4. **I7 is amended: ready waits on the workspace tree and the ready window**,
   the threads active in the last 7 days. The backfill takes the workspace
   first, then threads newest first, so both build early. Older threads fill
   in after. Until a thread's tree is built, its view and `zoom` read raw
   entries: the fallback I7 already gave an unbuilt leaf. A live event in it
   puts it at the front of the queue. Ready survives a restart, and a
   restart keeps filling the threads never drained to the end.

### Names kept

`TreeBackfillCompleted` and the `backfilled_at` column now mark the ready
point, not the last tree built. The event is persisted and a trigger can
subscribe to it, and the column would need a migration, so both keep their
names. The engine's own helpers say what they mean: `mark_ready`, `is_ready`.

### Rejected

- **Starting the workspace tree at a recent cutoff.** The workspace view and
  `find` would lose all older history.
- **Keeping turn leaves on thread lines.** Usable would stay close to complete,
  since every thread's leaves would have to build first.
- **Pipelining a thread's leaves.** It shortens the longest thread's chain but
  changes what every leaf reads. The chain bounds only the time to complete.

### What it costs

A backfilled turn line is written from raw entries in one step rather than
from summary lines. A long entry contributes only its head and tail to it.
Thread trees and `zoom` stay lossless. Calls and cost do not change. At 64
calls at once, the dev workspace is usable in 19 to 92 minutes, down from 17
to 79 hours. It completes in 1.1 to 6 hours.

## Amendment, 2026-10-07: the home thread anchors threadless spend

A model call no thread caused records its cost on the home thread, which is
created hidden while the switch is off ([ADR 0381](0381-one-model-call-service-records-every-call.md)).
The compactor's artifact leaves are the largest case. A hidden home thread is
one more, empty, tree in the backfill count.

## Amendment, 2026-10-07: a Tree turn is not offered the Classic `memory` tool

A ready Tree workspace was still offered Classic's `memory` tool. Its `correct`
edits `memory_entries`, which a Tree turn never reads, so the agent reported a
correction that changed nothing. The tool's own summary also promised memories
injected before every turn.

So the `memory` family is gated on a new `Gate::MemoryClassic`, the complement
of `Gate::MemoryTree`. Gated manifest tools now keep their manifest position, so
the module swaps `memory` for `recall` in one slot and moves no other tool. The
cached system prompt names `recall` in its lookup hint, and on Tree its
corrections section says the user's newer words are the correction.

A correction on Tree is therefore OptChat's: nothing is rewritten, and the
latest ruling holds. Extraction into `memory_entries` keeps running, so a
workspace switched back to Classic finds its store current. The `lucidos memory`
commands stay, as reads of that store.
