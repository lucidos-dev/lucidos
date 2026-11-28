# 0631: A deep link claims its target turn and pages history toward it, never a render-all

- **Status**: Accepted. Supersedes the deep-link half of ADR 0232's render-all.
- **Date**: 2026-11-08

## Context

The iOS home-screen PWA "randomly reloaded" and then showed the *Started on
Home* toast. Each one was a WebKit content-process kill, logged as a
`likely_crash` startup. Both kills in one long thread followed a notification
tap into it:

| Tap | Rendered |
|---|---|
| first | 9,520 events, 255 turns, a 245,421 px transcript |
| second | about 29,000 DOM nodes, against about 1,600 elsewhere |

A deep link set a *render-all*. The transcript drew `WHOLE_THREAD` and fetched
every event the server held. ADR 0232 made that claim last one visit, but
within the visit a long thread was still drawn whole. A phone cannot hold that.

ADR 0232 rejected dropping the render-all, because the reader would snap back
to the tail mid-read. That guarantee still binds.

## Decision

A deep link claims the turn that holds its target. The *render window*'s edge
moves to that turn, drawn whole, and never above the edge already drawn.
History loads one page at a time until the target's turn is loaded.

A link no loaded turn holds keeps the window as it is, once history runs out.
Its deadline reports it unresolved, as before.

The scroll-to-top chevron keeps its render-all.

## Rationale

The snap-back ADR 0232 feared came from the window re-seeding to the newest
turns. A floor at the target turn prevents it as well as a floor at the first
turn does. Turns older than the target do not help the reader land, and they
were the whole memory cost.

Paging toward the target bounds the fetch by where the target sits. A target on
the newest page fetches nothing more.

The chevron is different. It is a deliberate request for the first turn, and
the reader watches it work. Windowing it would answer a different question.

## Consequences

- **The claim is stored by key, as `claimed`** (`StoredWindowKind`). A fold of
  older history prepends turns, so an index would drift. A fresh open drops the
  claim and re-seeds, as ADR 0232 decided. A reader-grown edge still survives.
- **The claim is computed in render** (`deepLinkEdge` in `threadWindow.ts`),
  once the window can seed. Stored earlier, it would name a turn from the
  previous thread's list.
- **A deep-link page takes a hold that names no window key** (`readerOnlyHold`).
  A keyed hold makes the backfill re-point grow the window one step per fold
  and store it as `grown`. That would draw more turns on every page and outlive
  the visit. The stored edge already resolves its turn by key, so nothing needs
  re-pointing.
- **No render-all fallback.** Every `data-event-id` and `data-change-id` stamp
  comes from a turn, and `deepLinkTargetIndex` scans those same turns. So when
  it finds none, no node can match, and drawing the whole thread only rebuilds
  the crash before the same deadline toast.
- **A step of a *continuation fragment* does not count as found.** A fragment
  stamps no starter, so paging goes on until its real turn merges in.
- **A fresh open re-seeds a stale claim even when a link opens it.** The link
  claims its own turn after the re-seed. A chevron's render-all from an earlier
  visit would otherwise survive and draw the thread whole.
- **A page in flight keeps the link's deadline elastic**
  (`threadEventsStillArriving` counts a history read). Paging toward an old
  target can outrun the resolve deadline, which would clear the link mid-way.
- **A page that adds nothing ends the paging for that visit**
  (`linkPagingGaveUp`). Otherwise a server that reports older events it never
  returns would loop.
- **Not covered here:** `store/actions/event-navigation.ts` still calls
  `ensureWholeThreadLoaded` for an event older than the newest page. That
  fetches the whole history into memory, but the render windows from the
  target, so the DOM stays small.
- A reader who scrolls up from the target grows the window as usual.

## Alternatives considered

**Keep the render-all and virtualize the transcript, or give turns
`content-visibility: auto`.** The general lever for a reader who scrolls up
through a whole long thread, or taps the chevron. A much larger change, and not
ruled out. It does not remove the case for windowing a deep link, which never
needs the older turns at all.

**Fetch the whole history, but draw only from the target.** Simpler, since
`ensureWholeThreadLoaded` exists. Rejected: the fetch alone costs seconds and
memory on a 13,000-event thread, for turns the reader never asked to see.

**Window the chevron too.** Rejected: the chevron's purpose is the first turn,
so a window short of it would land the reader in the wrong place.
