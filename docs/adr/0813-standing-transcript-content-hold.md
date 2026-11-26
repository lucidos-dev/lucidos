# 0813: Hold the transcript reading line across content growth while the reader is idle

- **Status**: Accepted
- **Date**: 2026-11-25

## Context

A restored place or landed deep link held its target for a limited time.
Growth after that window moved the reading line. Content growth also moved
any reader who had reached their line by scrolling.

## Decision

Hold the reading line across resize rounds while the reader owns the
position and input is idle. History folds and turn controls use the same
writer for their own corrections. Remove the old arrival timers and the
image-loading scan. If a separate layout correction arrives during a hold,
apply its shift where the reader stops. A newer app write supersedes it.

After a finger lifts, keep coasting until scroll events go quiet. Retain
the timeout for a missing lift.

## Rationale

The line gives the hold a stable viewport offset when content grows earlier
in the transcript. It also gives the hold a way to detect corrections that
the history fold or turn control already made. An input hold takes priority
because an app scroll write can stop a native fling.

## Consequences

The line stays put even if an image decodes long after the reader arrived.
The saved-position restore still reports how it landed. It has no timed
settlement to report. Browser emulation exercises the scroll paths. A real
iPhone must still confirm that the hold leaves native momentum alone.

## Alternatives considered

- Extend the arrival timer. A longer window still expires and still omits
  readers who reached a line by scrolling.
- Correct during momentum. A correction would write the position while the
  platform is still moving it.
