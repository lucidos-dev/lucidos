# 0643: The widget fit check: an open Lucidos window renders each declared size, and the tool result says what each box cuts off

- **Status**: Accepted. Builds on the one widget box of [ADR 0621](0621-widgets-one-box-no-pin.md) and the XL size of [ADR 0633](0633-xl-widget-size.md)
- **Date**: 2026-11-10

## Context

The chat agent builds widgets it never sees. Four live runs each built a "Trip
countdown" widget. Each time, the large box cut off its flight rows on the desk
tile, the phone column, or both.

The agent had put `white-space: nowrap` and `text-overflow: ellipsis` on data
text. A phone is worse: a narrow column draws smaller cells, while text grows
with the UI scale. The host clipped sideways with no fade, so nobody was told.

Plan: `docs/plans/2026-11-10-widget-fit-check.md`.

## Decision

**An open Lucidos window renders the check, and the engine writes the note.**

- The engine emits a transient `WidgetFitRequested` scoped to one device whose
  window is visible. That page mounts the real `WidgetBox` off screen, one per
  surface, and walks each declared size.
- The surfaces are the desk tile (the full cell) and a 360 CSS px phone column.
  The scale is the largest `ui-scale` any registered device uses.
- Inside each frame the SDK waits for the page to settle, then reports text,
  media and ellipses the box cuts off. The page posts the reports to
  `POST /api/v1/widgets/fit-report`. An answer from another device is refused.
- The note, `[WIDGET FIT] <id>: ...`, rides the tool result of `create_app`
  with `kind=widget`, `write_file` and `edit_file` on a widget's own files, and
  `run_python`. It runs once per widget per call.
- `widgets(action="fit")`, `lucidos widgets fit` and `POST /api/v1/widgets/fit`
  run it on demand and answer the same note.
- The host also fades the right edge when content is wider than the box.

**The check never refuses or changes a write, and an unknown is never a pass.**
No visible window, no answer in time, or a box that could not be measured
says `not checked`, with the box rules beside it. The engine waits 6 s per
declared size plus 6 s, since each surface walks its sizes in turn.

## Rationale

- **Every client already has a renderer.** The DMG's WebKit window, a browser
  tab and the phone all lay out with the user's own engine and fonts. WebKit is
  what the iPhone runs, so a Mac check is close to a phone one.
- **One render path.** The probe mounts `WidgetBox` itself, and a twin test
  holds its px cell to the CSS formula. A second box would drift from the one
  the user sees.
- **A note, not a gate.** The write is the user's work. A check that blocked it
  would fail on a busy host or a closed laptop, so it informs and the agent
  decides. `[APP LINT]` set that precedent.
- **The largest scale covers every device.** A widget that fits there fits at
  every smaller scale, so one check stands for all.
- **The engine words the note.** The page sends measurements only, so the
  wording lives in one place and three surfaces answer the same text.

## Consequences

- A turn with no visible window, such as a trigger at night, gets `not checked`
  and the box rules. The agent still learns the contract.
- The check renders on one device's fonts. A Mac and a phone may lay out a few
  pixels apart; the narrow column and the largest scale leave margin for that.
- A coding agent that writes widget files with its own tools gets no automatic
  note. It runs `lucidos widgets fit`.
- A widget with a required param renders its no-params face, since a new widget
  has no instance yet. Its note says `not checked with params`, never a pass.
- `building-an-app.md` gains box rule 8: never ellipsize data text in a widget.

## Alternatives considered

- **Headless Chrome in the engine.** The first draft. It needs Chrome, which
  the DMG does not ship, plus a profile, certificates and a page token. The
  user chose the open window instead.
- **Refusing a write that does not fit.** It would turn a layout hint into a
  failed turn, and a closed laptop would refuse every widget write.
- **Each device at its own scale and width.** More renders for little gain:
  the worst case covers them.
- **Measuring in the engine from the HTML.** Layout depends on fonts, the UI
  scale and the theme. Only a real renderer knows what it cut.

## Amendment: an explicitly selected agent renderer

A local-token automation window has a logical device id but cannot answer as
an attested Device (ADR 0647). It may instead register a fit renderer session
under a verified origin token for the exact source thread. The engine binds
that session to its workspace, thread, widget and logical device, for one
selected fit request. The ordinary Device path is unchanged.

The launcher holds a fit-only credential in memory. Only its public selection
handle reaches the shell and the transient event. The credential belongs on
the exact top-frame fit-report request, never in an app frame, URL, event,
artifact or log. It is not an actor or an owner credential. Registration and
selection still require the verified source; reporting still owes the usual
door credential. Local-token-only reporting cannot answer a Device check.

Sessions expire after two minutes; there are at most 128 per engine, and each
answers once. Canceling the wait or restarting invalidates its credential.
Wrong-session or wrong-device reports leave the genuine check open. The real
shell and SDK still render both surfaces at every declared size and the largest
UI scale. The engine still writes all measurement and params caveats.

The credential proves the responder, never the truth of client measurements.
Lucidos neither ships Chrome nor starts a headless renderer in the engine.
