# 0718: Ordinary packaged closes retain every app window

- **Status**: Accepted
- **Date**: 2026-11-16

## Context

Cmd-W and the red close button hid `main` but destroyed secondary app windows.
The internal label determined whether page state and workspace connections
survived. Close to Menu Bar already retained every app window (ADR 0141).

## Decision

An ordinary packaged close hides any top-level Lucidos app window and retains
its page. Individual close and Close to Menu Bar share the hide operation,
including per-window inactive presence and watchdog hide intent.

Deliberate teardown bypasses retention. Development and auxiliary windows
keep their existing close behavior.

## Rationale

Window creation order should not determine lifetime. Retaining live windows
preserves page state and the existing workspace notification connection.
It also keeps session capture truthful without a second hidden-window registry.

The last visible app window entering the tray changes activation policy.
Reopen reuses retained windows. Notification taps still target one workspace.

## Consequences

Every packaged macOS user gets the same close semantics for local and paired
workspaces. Browser and development lifecycles are unchanged.
Hidden secondary webviews retain memory and connections, matching the tradeoff
already accepted for Close to Menu Bar.

Retention does not guarantee WebKit execution or native banner delivery.
The notification miss that prompted investigation remains unattributed.

## Alternatives considered

- Keep the first-window exception: rejected because an invisible label would
  still determine page lifetime.
- Destroy all windows and reconstruct them on reopen: rejected because it
  discards page state and existing connections.
- Add an independent notification receiver: outside this close-lifecycle change;
  it requires a separate delivery and subscription design.
