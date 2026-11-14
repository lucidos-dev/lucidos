# 0355: A failed side question is retried under its own id, so the card keeps its place; only an id whose every ask failed may be asked again

- **Status**: Accepted
- **Date**: 2026-10-03
- **Amends**: [0320](0320-side-questions-are-thread-events.md)

## Context

A failed side-question card had no way to try again. The user asked for a
Retry with a clean UI. Their card read "Load failed", which is Safari's text
for a dropped request. The engine runs each ask detached from its request
(ADR 0320), so it was often still answering while the card showed a failure.

ADR 0320 let one id be asked once. A second ask with the same id got a 409.

## Decision

Retry re-asks under the card's own id. The engine admits an id that was never
asked, or whose every ask ended in `SideQuestionFailed` with no answer. An id
still running, or answered, is still refused with a 409.

On the client, a local failure never overrides a recorded ask. Only a local
answer, or a retry not recorded yet, draws ahead of the events.

## Rationale

1. **The card keeps its place and identity.** A retry is the same question, so
   it stays where it was asked, on every device.
2. **No new event type.** A second `SideQuestionAsked` for an id is the retry.
   Every reader that excludes side-question events still excludes it.
3. **Admission stays safe.** The check still runs under `ASK_ADMISSION`, so two
   requests never both start one ask. A running or answered id cannot run twice.

## Consequences

- An id can carry several asks. The card counts them (`asks`), and the client
  uses that count to tell a retry it sent from one already recorded.
- Startup recovery counts asks against settlements per id, so a retry a
  restart interrupted also fails, and its card offers Retry again.
- A retried card that was folded keeps its fold.
- A quick failure retries on its own, on the schedule every send shares
  (`SEND_RETRY_BACKOFF_MS`, three retries), before the card shows Retry. Only a dropped request or a 5xx that came back within 20
  seconds qualifies, so a refusal or a timed-out answer shows at once. A
  dropped first request makes most failures look instant on a phone.

## Alternatives considered

- **Retry under a new id.** Frontend only, but the new card lands at the bottom
  of the thread and the failed one stays behind, folded or not. Two cards for
  one question is the clutter the user asked us to avoid.
- **A `SideQuestionRetried` event linking the new id to the old.** It would hide
  the old card, but adds an event type every generic reader must exclude, for
  the same result as reusing the id.
