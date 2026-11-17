# 0660: A hidden page in a non-key window is parked, and a reload is judged by a new document's beat, amending 0180

- **Status**: Accepted
- **Date**: 2026-11-11

## Context

A development window logged `WKWebView heartbeat timeout ... while the window is
on screen: reloading` once a minute for nearly two hours. WebKit's own log for
that process shows why.

Another window covered it, and WebKit reported activity state
`IsVisibleOrOccluded` without `IsVisible`. Four minutes later it suspended the
process. Timers stopped, so the heartbeat stopped. Every reload resumed the
process for about three seconds, and WebKit suspended it again. The loop ended
when the window was uncovered.

Two defects combined:

1. ADR 0180 reads a covered window as on screen, which is right for a window the
   user is clicking. For a covered window it is wrong: the silence is WebKit's
   suspension, and a reload cannot undo it.
2. The futility test compared the heartbeat count. A resumed process lets the
   OLD document send one overdue beat before the reload commits. The count
   moved, so every reload looked successful and the backoff never started.

## Decision

The heartbeat carries two optional arguments: `page`, an id minted once per
document, and `visible`, the page's `document.visibilityState`. The page beats
on its 15 s interval and on `visibilitychange`.

- A window is parked when its page last reported hidden AND the window is not
  the key window (`window_screen::page_should_be_running`). The key window is
  always watched, whatever its page says.
- A reload is futile when the beating document is the same one that was beating
  at the last reload. A page that sends no id falls back to the beat count, and
  a page that sends no visibility counts as visible.

ADR 0180's decision stands: on screen is still the client's own fact, and
`isVisible` is still not asked.

## Rationale

The page is the one source that knows what WebKit decided, and it reports
through a standard API with no new AppKit surface. Restricting the park to a
non-key window keeps ADR 0180's recovery: a window the user clicks is the key
window, so a wrong "hidden" cannot strand it. The window that WebKit suspends
because it is covered is never the one being clicked.

A per-document id is exact where a count is not. It separates a stale beat from
a reload that worked, with no timing window to tune.

## Consequences

- A covered window keeps its page state until it is uncovered or clicked. Nothing
  is re-fetched, which for a remote workspace also spares the relay.
- A hidden page in a window that stays non-key and never resumes is not reloaded.
  That is intended: WebKit resumes it when it is uncovered.
- A page that crashes or hangs after it reported hidden cannot report visible
  again. Its window stays parked until the user makes it key, even once it is
  uncovered. Clicking it starts the silence clock, so recovery takes one timeout
  from that click. We accept this: the dead page is visible to the user, and
  the click that ADR 0180 protects still recovers it.
- A remote gateway on an older release sends neither argument. It keeps the old
  behaviour for covered windows, bounded by the existing backoff.
- The `heartbeat` command gains optional arguments only. The ACL grant is
  unchanged.

## Alternatives considered

- **Ask AppKit for occlusion** (`occlusionState`). Rejected by ADR 0180 for
  lying about a window the user was clicking. The page's report, gated on key
  status, reaches the same answer from the source that acts on it.
- **Stop WebKit suspending occluded pages.** There is no supported switch, and
  suspension is correct behaviour that saves memory.
- **Treat any silence on a non-key window as parked.** A frozen background
  window would then never recover. The page's own hidden report is the evidence
  that the silence is suspension.
- **Widen the beat-count test with a grace period.** The stale beat arrives
  within a second of the reload, so a timing window tunes around the symptom.
