# 0258: The transcript scrollbar measures the drawn slice, because no estimate of undrawn history is right

- **Status**: Accepted
- **Date**: 2026-09-23
- **Supersedes**: [0253](0253-the-transcript-scrollbar-is-drawn.md)

## Context

ADR 0253 drew the transcript scrollbar in both layouts, so the thumb could
report position in the whole thread. Its range was the drawn slice plus a guess
at the undrawn history: the drawn slice's height per event, times the events not
drawn. The server counted the events behind each page (`olderCount`) so the
guess could cover history not yet loaded.

Reported on a coding-agent thread of 3,610 events in 8 turns: the thumb opened
small, grew later, and sometimes locked during a drag.

- Settled turns fold hundreds of tool steps into a few rows. An event there
  costs almost no height, so the guess ran 5 to 10 times too tall. The thumb
  opened near its 24px floor and grew as real turns replaced the guess.
- A drag into the guessed head parked the transcript at the top of the drawn
  slice, by design. Older pages load on a scroll event, and a drag pinned at the
  top fires none. So the thumb stopped following the pointer.

The rest of the module existed to hide the guess's error:

- a held head, and a walk back to the estimate on an upward scroll;
- a bisection, so the walk never moved the thumb against the scroll;
- a re-base at each anchor write;
- drag parking, wheel forwarding and a gesture stamp.

It took nine fixes in one day, each for a new way the guess showed.

## Decision

The transcript scrollbar measures the drawn slice. Desktop uses the native
scrollbar. Mobile keeps its drawn touch indicator, for the layout reason it
always had, and draws it from `scrollTop`, `scrollHeight` and `clientHeight`
alone. The events endpoint no longer counts what sits behind a page.

While the reader holds the native scrollbar, nothing lands above them: no
window grow, and no fetched page. Both wait for the release.

## Rationale

The client cannot know the height of content it has not drawn. Folds, the steps
toggle and prose length each move it by 10 times or more per event. So no
count-based guess is right on every thread. A guess that is wrong needs
compensation, and the compensation was where the bugs lived.

Measuring the drawn slice is the infinite-scroll pattern chat transcripts
commonly use. It is exact about what it describes, and the native scrollbar
needs no code of ours to drag, press or wheel.

## Consequences

- On a windowed or paged thread the thumb describes the drawn slice. When the
  reader comes back deep into a long thread, the thumb can sit near the top.
  The up chevron still says when there is more above, from the window and the
  server's `hasMore`.
- The thumb jumps and shrinks when older turns draw or a page lands. The
  CONTENT does not: the anchor write and the history hold keep the reader still.
  `transcript-native-scrollbar-holds-the-content-desktop.spec.ts` pins that for
  a real drag on the native thumb.
- The scrollbar hold exists because Chromium puts its own drag position back,
  with no input event, which undoes an anchor write. A page may still be
  fetched during the scrollbar hold, but it lands only a frame after the
  release, at the store's landing gate (`scrollbarReleased`). A grow the
  scrollbar hold put off runs then too (`onScrollbarReleased`). A drag pinned at
  the top fires no scroll event, so without the release it locked.
- A scrollbar hold starts on a primary press on the scroller's own box
  (`isScrollbarHeld`). That covers a classic gutter, and an overlay thumb, which
  sits inside the client box. WebKit's overlay scrollbar (Safari, the macOS app)
  is unverified. If it sends the page no press, nothing is held there, and a
  grow can land under a drag as it did before.
- Detecting a press in the classic gutter still covers the standing follow, as
  it did before ADR 0253.

## Alternatives considered

**Keep the drawn thumb and guess in rendered rows.** Rows track height better
than raw events while steps are folded. Rejected: the guess stays a guess, the
unloaded history is still counted in raw events, and every piece of compensation
stays with it.

**Measure and remember turn heights.** Record each turn's height once drawn,
and guess only the rest. Rejected: a first open has measured nothing, so it
starts from a guess anyway, and the cache adds state keyed on folds and toggles.

**A spacer for the undrawn history.** Rejected in ADR 0253 for a reason that
still holds: every reader of `scrollTop` assumes the scroller's top is the drawn
slice's top.
