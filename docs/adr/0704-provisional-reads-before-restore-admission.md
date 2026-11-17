# 0704: A never-admitted page reads one thread's summary and events before restore admission; everything else stays gated

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

Restore admission holds every HTTP request until the stream's hello is checked
against this device's restore generation, which lives in IndexedDB. On the
iPhone PWA that database can hang right after a notification tap. The tapped
thread then stayed a skeleton, because its reads waited on storage. The owner
set the bar: the thread shows within 2 seconds, even when storage stalls.
Evidence and the stall analysis: `docs/plans/2026-11-15-notification-reads-before-restore-admission.md`.

## Decision

Until a document's first admission, a GET for `/api/v1/threads/<uuid>` or
`/api/v1/threads/<uuid>/events` skips the admission wait. We call it a
*provisional read*. Every other request keeps the gate.

## Rationale

- The engine fences only mutating requests by restore generation, so a GET
  sends no intent across a restore.
- The real hazard of an early read is state that restore capture later takes
  as this page's intent. A document never admitted holds no old-generation
  state. Its only capture-relevant read is a summary's compose fields, and
  `acceptComposeReport` ignores those until admission.
- Stream frames are dropped until admission, so a provisional read opens an
  event gap. Admission after one runs the full gap reconcile, which also loads
  the compose fields it skipped.
- Two URL shapes are enough to show a notification's thread, and small enough
  to audit.

## Consequences

- A notification tap reads its thread without device storage. Live updates,
  sends and the thread list still wait for admission.
- A page that was admitted and is reconnecting keeps the full gate.
- A new read added to the thread view does not become provisional by itself.
  It joins the allow-list only with the same capture analysis.

## Alternatives considered

- **Release every GET before admission.** It covers Home and the list too, but
  many reads feed state capture reads: compose selections, preferences, drafts.
  Auditing each one is open-ended, and one miss corrupts a restore.
- **Provisional reads on a reconnecting page too.** Its composer can hold
  old-generation text that capture must keep, and a read could overwrite it.
- **Fix only Retry.** The owner rejected it: within the stuck document, fresh
  connections kept failing for 20 to 60 seconds.
- **Time out admission and open the gate.** That would replay retained intent
  against a restored workspace, which the barrier exists to prevent.
