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

## Amendment, 2026-10-10: Home draws no shelf

The maintainer's decision. A widget pinned in Home already lives in the Home
long-press menu and its window, so a chip under Home's title only repeats it.
Home's title row draws no shelf, and its widget menu reads "Pin to Home" and
"Unpin from Home". Every other thread keeps its shelf and its drop under the
title, as above.

In Home, a plain tap on the Home icon has nowhere to navigate. So it opens the
same menu a hold does, and the widgets stay one tap away. The Lucidos menu's
Home row keeps navigating, since it also opens over the thread list.

## Amendment, 2026-10-10: three modes, resize, a remembered layout

The maintainer's decisions, on the Thread cost widget floating in a window.
Plan: `docs/plans/2026-10-10-widget-windows-minimal-and-resizable.md`.

- **Three *widget window modes*.** Minimal is the default on a desktop: the
  widget alone, in the shape it draws, with its controls floating over it on
  hover or after a tap. Window is the card with its bar, resizable from its
  corner, and it holds its size so a growing widget scrolls inside it. Docked
  is the default on a phone: hung under the thread's title row.
- **The widget is told its mode.** The host's `widget` push carries it, and
  the SDK stamps `data-widget-mode` on `<html>`. In minimal mode the SDK
  reports the content's width as well as its height. In any window mode it
  passes a press inside the frame up, so a touch screen can show the
  controls. The SDK module is renamed `widgetFrame`, since it now carries more
  than a height.
- **The layout outlives Close.** Each window's place, mode and size is its own
  per-device record, so a widget opens again as it was left. Unpinning the
  widget forgets it.
- **Windows float over the header chrome too.** Popovers and menus moved to a
  new `--z-popover` layer above the windows, so a menu never opens behind one.
  This replaces "under the header chrome" above.
- **Docking is a button, never a drag.** A drag to the top of the screen
  means "put it over the header" now, so it cannot also mean "dock".

The "No resize yet" consequence above no longer holds. Snap and keyboard
move are still not offered.
