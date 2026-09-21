# 0234: A position behind the loaded page opens at the newest page

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

A *reading position* names a turn (ADR 0152). A cold open reads the newest
`THREAD_EVENTS_PAGE_SIZE` (400) events rather than the whole thread (ADR 0230).
So a reader who walked far enough back leaves a record naming a turn the next
open does not load.

`reachAnchor` in `components/chat/ThreadView.tsx` looks that turn up in
`exchanges` and clears the restore target when it finds nothing. The walk grows
the render window and never fetches, so it stops at once and the thread opens at
the top of the newest page.

That was correct for a turn genuinely gone and undecided for a turn merely
behind the page. The 2026-09-21 nightly filed it, left
`e2e/transcript-reopens-on-the-same-turn.spec.ts` red rather than shrinking its
seed, and asked for a product decision.

## Decision

Such a position opens the thread at the top of the newest page. The app fetches
no older history to chase it. A position INSIDE the loaded pages is still
honoured exactly, which is the walk `reachAnchor` performs.

## Rationale

Paging exists to make a long thread open fast. A chase spends exactly what
paging bought: the open stops being one request and becomes a request per page
until the turn lands or the restore's ceiling expires. It is worst on a phone
over a slow link, which is the reader paging was added for.

The cost lands on one reader in one case: somebody who walked a long thread back
past its newest page loses their place on the next open. They keep every way
back the transcript already has, the scroll-up walk and the up chevron among
them. Nothing is lost that a gesture cannot reach.

## Consequences

- A reader parked behind the loaded page opens at the top of the newest page.
  Nothing tells them the record was not honoured, and nothing should: the open
  is the ordinary one.
- The record is NOT deleted. A client that does hold the history, or a later
  open after a backfill, still lands on the turn.
- "Not in the loaded window" is now a decision rather than an omission, so
  `reachAnchor` says so at the give-up.
- `e2e/transcript-reopens-on-the-same-turn.spec.ts` narrows to the in-page case
  and gains a second test for this one, which also asserts no older page is
  fetched. That second half is the guard against the chase returning.

## Alternatives considered

**Chase the history.** When the target is absent and `hasOlderEvents`, call
`requestBackfill` and keep the target. The existing re-point brings the walk
back, and `onRestoreSettled` bounds the chase at the restore's 20 second
ceiling. Rejected: it undoes the paging work on exactly the open paging was
added for. Not kept behind a flag either, since a flag is the chase plus a
second code path to maintain.

**Widen the first page until the position fits.** Rejected for the reason
ADR 0230 already gives: raising `THREAD_EVENTS_PAGE_SIZE` hides the shape rather
than fixing it, and no page size bounds how far back a reader can walk.

**Retire a position the page cannot hold.** Delete the record and open clean.
Rejected: it throws away a position a better-loaded client could still honour,
and it buys nothing. Where the thread opens is identical either way.

**Tell the reader.** A toast saying the place could not be reached. Rejected as
noise on an ordinary open, for a reader who can scroll back.
