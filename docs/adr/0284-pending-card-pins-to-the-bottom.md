# 0284: A card awaiting the user draws at the bottom; once resolved it sits at its resolution point, in every lane

- **Status**: Accepted
- **Date**: 2026-09-25

## Context

A question or permission card reads "Needs your answer" until the user acts.
Since ADR 0255 an engine re-entry, such as a sub-thread's report, keeps the
question live and waits behind it. That report opened an exchange BELOW the
card. So the one thing the reader had to act on scrolled away from the composer
that answers it.

Resolution already moved a chat card to its resolution point. A coding-agent
card stayed where it was asked. Its continuation follows `current` rather than
a request id, and moving the card alone would have stranded it.

## Decision

While a card reads "Needs your answer", the transcript draws it last, above
only the queued-message group. Once resolved, the fold moves it to its
resolution point and hands it `current`, in every lane.

## Rationale

The card is the thread's call to action, so it belongs next to the composer.
Its answer is the moment the reader engages with it. Placing it there makes
the timeline read in the order things happened to the reader: the callback
that landed while they were away, then the answer, then the work it started.

The pin is render order only. `exchangeStatus` still reads the fold order, so
the callback beneath the card still reads "Held until you reply". Handing
`current` over is what makes the coding-agent move safe: the continuation
lands inside the answered card instead of above it.

## Consequences

- An answered card does not jump. It was drawn last while pending, and it is
  last in the fold once answered, unless the caller spoke after it.
- A caller's utterance still stops the move (ADR 0201). A card with spoken
  words after it keeps its ask position once answered.
- A stale coding-agent card stays where it was asked. The engine's cleanup
  sweeps resolve cards the agent already moved past, after a restart or at
  idle. The fold and the pin both read agent progress in or after the card as
  that sign, since a live card blocks its turn. Such a card is never pinned.
- A pinned card ignores the window floor, like the queued group.
- `McpConsentRequested` never resolves in the event union, so it is not pinned.

## Alternatives considered

- **A sticky overlay above the composer.** Rejected: it duplicates the card and
  covers the transcript, and the card is already an ordinary node that can
  be drawn last.
- **Pin in the fold instead of the render.** Rejected: `isLast` and the `held`
  status read fold position. Moving the card there flips the callback beneath
  it from "Held until you reply" to a settled state.
- **Pin questions only.** Rejected: a permission card reads the same label and
  blocks the same way.
