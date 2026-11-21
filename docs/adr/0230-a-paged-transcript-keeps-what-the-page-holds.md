# 0230: A paged transcript keeps what the page holds, and the client fixes it

- **Status**: Accepted
- **Date**: 2026-09-20

## Context

A cold open reads the newest `THREAD_EVENTS_PAGE_SIZE` (400) events rather than
the whole thread. That cut a reported 12,982-event open from seven megabytes to
a page.

The exchange fold was never told. It drops any step arriving before the first
exchange-start event it can see, which was unreachable while a client always
held event 1. A page starting mid-turn makes it the common case.

A reported thread of 1,062 events measured like this, folded through
`computeExchanges`:

| Loaded | Events | Exchanges | Rendered rows |
|---|---|---|---|
| page 1 | 400 | 2 | 28 |
| page 1 + 2 | 800 | 2 | 28 |
| whole thread | 1,062 | 3 | 529 |
| page 2 alone | 400 | 0 | 0 |

Twenty-eight rows is shorter than a phone pane, so the transcript never
overflowed. A transcript that does not overflow fires no scroll event, and the
scroll handler was the only caller of the backfill. The up chevron asked the
same question off the same loaded set and hid itself. The thread was stuck with
no gesture and no button, and 662 events on the server.

## Decision

A page renders what it holds. The fold opens a *continuation fragment* for
steps whose boundary is older than the page. The render window's fill escalates
to fetching a page when it has nothing left to grow. Both halves are
client-side, and the events endpoint and the page size are unchanged.

## Rationale

The fold is where content was lost, so it is where content is kept. Everything
downstream was correct about a set it had already been handed short.

Paging's own code had assumed the fragment existed: `PendingBackfill.nextKey`
is documented as covering "a fragment whose opening message is still
unfetched". The fold produced no such thing. Building it makes that assumption
true rather than working around its absence.

The escalation stays because a fragment is not a guarantee of height. A pane
taller than one page's rendered rows, or a page of pure metadata, still leaves
the reader unable to scroll. Reachability is the invariant, and measurement is
the only honest test of it.

## Consequences

- "Is there anything above?" now has one answer, the thread's, not the loaded
  set's. Three readers were asking it the narrow way: the fill, the up chevron,
  and the chevron's own handler.
- A fragment is a turn whose kind cannot be read off `userEvent.type`. Every
  reader that switches on that type has to know. `canQueueBehind` did not, and
  handed a chat follow-up the running turn's stream.
- A page that draws no height costs a request. `MAX_FILL_BACKFILLS` bounds it,
  and the chevron remains the way out at the cap.
- The corrupt-thread state survives: a fragment opens only while the thread
  reports older events unloaded.

## Alternatives considered

**Extend the page backwards to a turn boundary, in the engine.** One request,
no fragment, and the fold untouched. Rejected on two counts. The engine has no
notion of an exchange-start event: `EXCHANGE_START_TYPES` lives only in
`exchange-grouping.ts`, so this mirrors that set into Rust and opens a drift
seam across the boundary. And it lets one turn dictate the page size, which the
reported thread would have taken to 999 events.

**Keep paging on the client until a boundary is loaded.** The small loader fix,
needing no fold change and no new render path. Rejected: a running coding-agent
turn is routinely hundreds of events, so every open of such a thread would
fetch the whole turn. That is the cost paging was added to remove, reappearing
on exactly the threads it was added for.

**Raise `THREAD_EVENTS_PAGE_SIZE`.** Rejected as hiding the shape rather than
fixing it. The reported thread still drew 28 rows at 800 events, because the
turn behind the page was longer than any page worth loading.

**Leave the fold alone and rely on the escalation.** Two fewer files changed,
and it does unstick the reported thread. Rejected on two counts. It pays for
the fix in round trips on every thread of that shape. And it leaves a page with
no boundary at all folding to zero exchanges, which the transcript reports as
corruption on a healthy thread.
