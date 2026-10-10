# 0299: Surfaces open in three places: at their trigger, hung from the header over their pane, or as a blocking dialog; the explainer moves to its trigger

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

ADR 0290 gave every surface one box, but not one place to open or one inset.
The user reported that surfaces "pop up at different places" and do not share a
style. An inventory of all 26 overlay surfaces found:

- Search Everywhere centred on the window, so on desktop it straddled the pane
  divider. The Lucidos menu centred on the Conversation pane. File search sat
  2rem from the top of the window, inside the header.
- The explainer opened as a centred modal with a scrim. Every other popover
  opens at its trigger, the Waits panel included.
- Heads inset text by 0.75rem and toasts by 0.875rem. Six surfaces drew their
  own header with their own literals.

The plan is `docs/plans/2026-09-27-surface-placement-and-inset.md`.

## Decision

A surface opens in one of three places, chosen by what opened it:

| Family | Where it opens | Members |
|---|---|---|
| Anchored popover | At its trigger, clamped inside the trigger's pane | Explainers, Waits, Todos, background activity, the route panel, every menu |
| Header palette | Hung under the header on one line, centred on the pane whose header holds its button | The Lucidos menu, Search Everywhere, file search |
| Blocking dialog | Centred in the window, or top-aligned over the Conversation pane for transcript detail | Confirm, prompt, progress, release notice, the step detail family |

Every surface with a head, body or foot insets its text by one token,
`--surface-inset` (1rem). Menus wear the frame alone, `.surface-box`, and keep
their compact rows and buttons.

## Rationale

- **Placement says where the surface came from.** A panel that opens beside
  the control that opened it needs no explanation. A panel that opens somewhere
  else makes the reader look for it.
- **The explainer's reasons for a dialog were solved elsewhere.** It rejected
  an anchored popover because it would clip in a narrow pane, clip at a phone's
  edge, and need its own scroller. The Waits panel solved all three: the
  position hook clamps to a container and publishes the width that fits, and
  the popover body scrolls. A dialog's scrim also hid the icon, so the reader
  lost track of what they had asked about.
- **A palette belongs to its pane.** Centred on the window, a palette opened
  from one pane lands half over the other. `usePaneCentre` measures the pane
  under the button and `.surface-pane-centred` shifts the panel over it. The
  transcript modals already used the same shift.
- **One inset token keeps heads and bodies aligned by construction.** A rule
  that restates the value to line up with the head stops matching the day the
  inset changes.

## Consequences

- An explainer no longer dims the app. The UI behind is still inert, and the
  popover still traps focus, closes on Escape and swallows the outside click.
- Search Everywhere opened by shortcut has no button, so it centres on the
  focused pane group. File search with no button falls back to the Canvas pane.
- A palette narrows to fit its pane, 1rem clear of each side, so it never
  reaches the divider. The transcript modals keep their width, and may
  overhang a narrow pane, with the clamp keeping them inside the window.
- The split-button menu keeps its own frosted box. It is docked to its button,
  with no bottom border and an upward shadow, so it is not a floating surface.

## Alternatives considered

- **Keep the explainer a centred dialog, only pad it more.** That was the first
  ask. It left the explainer the one popover in the app that did not open where
  it was clicked, which is what the user raised next.
- **Centre every palette on the window.** One rule, no measurement. It keeps
  Search Everywhere straddling the divider, and it would move the Lucidos menu
  off the mark that opens it.
- **Nest the style editor's fields in a body wrapper.** Its panel is the
  scroller and its instruction is the one child that gives up height. A wrapper
  would re-home both contracts, so its children take the inset as side margins
  instead.

## Amendment, 2026-10-02: a subscription's condition opens at its chip

The condition behind a "with a condition" chip moves from the blocking-dialog
family to the anchored popover family. It opened as transcript detail, pinned
to the top of the Conversation pane. The chip sits on a wait row, which is
usually the last thing in the transcript. So a press at the bottom of the pane
put a small panel at the top, and the user reported it.

The step detail family keeps its dialog. Those views carry a full tool call or
a diff and need the height. A condition is a few lines of JSON, and the
waiting panel already shows the same body inside a popover.
