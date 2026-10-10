# 0402: A widget is an app kind shown in a thread, with full app reach: same app frame, same data/apps/ folder, no artifact frame, typed spec or data/widgets/ root

- **Status**: Accepted. Placement amended by 0405: a widget opens from the shelf, and the transcript holds a one-line row
- **Date**: 2026-10-09

## Context

A tester asked for visual answers inside the chat: "Currently, I always build a
custom app for visualization when I could just have it in the same chat." A fare
comparison is too much for an app and too dense for text. Before this, a visual
answer was either markdown in the transcript or a full app under
`data/apps/<id>/`. Nothing sat between them.

The counterpoint was that many tasks deserve a persistent interface, not a
throwaway one. So the answer had to start small and grow, without a dead end.

Plan: `docs/plans/2026-10-09-in-thread-widgets.md`.

## Decision

A **widget** is an app whose app manifest says `kind: "widget"`. It lives in
`data/apps/<id>/`, runs in the app frame with the full bridged SDK, and reaches
exactly what an app reaches. Its app manifest records its origin thread and a
`reusable` flag.

It shows inline at the turn that made it, and as a chip on the thread's widget
shelf. The transcript stores only the app id, in a thread event. One
classifier in `core/apps.rs` decides app or widget, and every apps reader
filters through it.

Three tiers, each one step up: a widget, a reusable widget (the same files,
offered to every thread), and an app built from it by "Make app".

## Rationale

- **One owner type.** Every frame, bridge, capability, storage and reach rule is
  keyed on an app id. A widget that is an app inherits all of them, and none can
  drift. The ADR 0231 residuals apply to widgets exactly as to apps, and widgets
  add none.
- **The same build path.** A widget is made by `create_app` with the same
  knowhow, so it costs nothing new to learn or to maintain.
- **Durable, not listed.** "Ephemeral" means "not in the apps list", not "not
  stored". A widget survives reload, another device and compaction, because it
  derives from thread events plus files in git.
- **Rule 3 is met.** A widget is plain HTML/CSS/JS against the SDK. The shell
  gains no chart library.

## Consequences

- Delete (a thread) removes that thread's own non-reusable widgets, keeping
  ADR 0192's privacy promise. Reusable widgets, and apps made from a widget,
  stay. Nothing else removes a widget: "Hide from shelf" only writes an event.
- Every new apps reader must filter through the classifier, or a widget leaks
  into the apps list.
- A widget frame costs a renderer process (ADR 0227), so only visible or opened
  widget frames mount.

## Alternatives considered

- **An inline HTML artifact frame (ADR 0322).** It has no SDK, so a widget could
  not read live data or subscribe to SSE. Lost on reach.
- **A typed chart or table spec rendered by the shell.** Every new shape becomes
  engine and frontend work, and the shell needs a chart library. It also cannot
  hold a fare grid with options and toggles. Lost on expressiveness and rule 3.
- **A separate `data/widgets/` root.** It would give every frame, bridge,
  capability, reach and storage rule a second owner type to drift from. Lost on
  one owner type.
- **Thread events only, or a scratch file.** Events cannot hold a growing app,
  and `.lucidos/tmp/` may lose a scratch file at any time. Lost on durability.
- **Narrower reach for widgets.** Apps are made by the same agents the same way
  and open automatically. A narrower widget rule would be a second policy with
  no threat it stops. Per-app grants stay future work for both (ADR 0231).
