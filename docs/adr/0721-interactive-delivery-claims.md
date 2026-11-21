# 0721: Coordinate interactive presentation through shared device claims

- **Status**: Accepted
- **Date**: 2026-11-16

## Context
Native hide/show changes reach one page before its BroadcastChannel announcement reaches others.
Independent elections can therefore deliver an interactive request zero or two times.
Matching cached peer lists cannot guarantee coordinated presentation (ADR 0585).

## Decision
Use a workspace/device IndexedDB journal and exclusive Web Lock for automatic interactive presentation.
Persist pending requests, participating window views, claims and presentation receipts.
BroadcastChannel announces committed changes; it never authorizes presentation.
Page lifetime locks identify dead owners. Another participant adopts their pending work.

An SSE envelope event identity names navigation, push-enable prompts and OAuth completion toasts.
Forms use their persisted request id across live SSE and pending-request sync.
Resolution writes a tombstone even before request insertion.

Sync queues every open form, newest first, including while another form is busy.
It snapshots journal form identities before reading the server's pending list.
Check snapshot identities for persisted `FormRequestResolved` events through a bounded batch lookup.
Absence from the pending list is not resolution: archived threads also leave that list.
Suspend snapshot forms absent from the pending list; restore eligibility when they return.
Resolve the proven batch and update eligibility atomically before reconsidering pending work.

A separate workspace/device Web Lock serializes server reads through reconciliation commits.
Archive and unarchive frames refresh eligibility without waiting for reconnect.
Forms received during the server read remain untouched by reconciliation.
Notification payload event ids retain their tap-target meaning.

Filter current native visibility before content, authorization and recency preferences.
Keep the all-hidden fallback and browser/PWA eligibility policy.
Revalidate local state after asynchronous journal writes, immediately before synchronous UI acceptance.
Declines remain pending and exclude that participant revision until its state changes.

Persist a claim before presentation, then a receipt after acceptance.
Release the lock before waiting for an answer or external operation.
An abandoned claim becomes uncertain, even when its former owner remains alive.
Surface an explicit Retry action; never automatically replay an ambiguous side effect.
Capability or storage failures also surface an actionable error, without uncoordinated fallback.
Native visibility initialization settles before delivery opens a transaction, preserving actionable readiness errors.

Install the service synchronously; its methods await the asynchronously imported device adapter.
Teardown cancels queued calls and stops the adapter even when its import finishes later.

Preserve direct user opens, OAuth initiating-window focus authorization, native taps, captures and widget-fit targeting.
Keep unanswered forms and pending or uncertain requests. Bound completed receipts to 512.

## Rationale
Cold opens install the service and visibility bootstrap before SSE connects, so early requests cannot bypass coordination.
The journal and browser adapter load asynchronously; the initial module set retains the service proxy and routing guards.

A shared transaction and lock serialize claims across pages with divergent cached peer state.
Retaining pending work separates delivery from nomination. Uncertain recovery exposes the DOM/receipt boundary honestly.

## Alternatives considered
Separate BroadcastChannel elections depend on announcement order and reproduced both missing and duplicate presentation.
A new engine delivery service would add server state for a device-local coordination problem.
Automatic replay of abandoned claims could repeat an already accepted side effect.

## Consequences
Independent pages share one claim authority while retaining their own UI state.
Delayed announcements cannot discard pending work or permit a second accepted presentation within the retained receipt history.
No browser transaction can atomically commit both a DOM change and an IndexedDB receipt.
Explicit Retry can repeat a presentation after a crash; the warning states that uncertainty.
Receipt retention bounds duplicate suppression for completed requests.
No server database, service or migration is added.

Chromium tests exercise separate pages with real IndexedDB and Web Locks, including reload, dead owners and workspace/device boundaries.
The native bridge is simulated in these tests. Packaged close and OS-banner smoke checks remain separate manual checks.
