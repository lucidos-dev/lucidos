# 0419: Widget windows float over the app and stay open; the one surface outside the outside-tap rule

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

The maintainer asked for the widgets pinned to Home to be one long press away
from the Home entries, without opening Home first. On the approval card they
refined it twice: the widgets should float "above anything u do", and the user
should be able to open several and place them anywhere.

Every other surface over the app is an `<Overlay>`: an outside tap closes it and
is swallowed, and the shell behind it goes inert
(`.claude/rules/frontend.md` § Modals & Popovers). A widget that closes on the
next tap cannot float above what the user does.

Plan: `docs/plans/2026-10-10-home-long-press-opens-pinned-widgets.md`.

## Decision

A hold or a right-click on a Home entry lists Home's pinned widgets. A pick
opens a *widget window*: the shelf's open widget card, floating over the app.
Several can be open, one per widget. Each is dragged by its bar, raised by a
press, and shut only by its own Close. The open set and the positions persist
per device.

Widget windows are not overlays. They take no outside-tap dismiss, no inert
shell and no Escape. They are the one recorded exception to the overlay rule.

## Rationale

- **"Above anything you do" means the app stays live under it.** Inerting the
  shell, or closing on the next tap, is exactly what the maintainer asked
  against.
- **One card, two hosts.** The window renders the shelf drop's
  `OpenShelfWidget`, so a widget looks and acts the same wherever it opens.
- **Under the header chrome, over the panes.** `--z-widget-window` sits below
  `--z-control-panel`, so menus, modals and toasts still open above a window
  and the header stays reachable. A press on a window while a menu is open is
  an ordinary outside tap for that menu.
- **Stacking by `z-index`, never by DOM order.** Moving an iframe in the DOM
  reloads it, so raising a window must not reorder the list.
- **Per device, in `localStorage`.** A phone and a desktop want different
  layouts, and the focused thread already persists the same way. Each entry
  carries its thread id, so another workspace on the same origin draws nothing.

## Consequences

- `frontend.md` names the exception. A new surface that wants to stay open
  cites this ADR or argues its own.
- Home's widgets are read once per page load, so SSE keeps the menu and the
  windows fresh from any thread.
- A widget unpinned from Home closes its window.
- Shelf chips in a thread keep their drop under the title (ADR 0407).
- No resize, snap or keyboard move yet.

## Alternatives considered

- **Open the widget in Home.** The first plan: navigate to Home and drop the
  widget open under its title. Lost: the maintainer wanted to stay where they
  were.
- **One floating drop that closes on an outside tap.** The second plan, an
  `<Overlay>` under the header. Lost: it cannot stay up while the user works,
  and it holds one widget.
- **Shelf chips open windows too.** Offered as a fork on the approval card and
  not taken. A thread's chips stay with that thread's title.
- **Sync the layout through preferences.** Lost: positions measured on one
  screen are wrong on another.
