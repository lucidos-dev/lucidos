# 0335: A backdrop dialog contains Tab; an anchored popover closes on a Tab from outside it

- **Status**: Accepted
- **Date**: 2026-09-30

## Context

`<Overlay>` makes the shell behind it inert with `pointer-events`, which stops
the pointer and nothing else. The per-pane Tab trap stood aside for every open
overlay, so Tab was native there and walked into the shell. Only five dialogs
trapped Tab by hand. A step detail modal opened from the keyboard left focus on
the step row, and Tab went on through the transcript underneath it. A free-text
dropdown opened its menu on focus, and tabbing past it left the menu open.

The audit and the fix are in `docs/plans/2026-09-30-keyboard-tab-audit-fixes.md`.

## Decision

The document Tab branch routes the key through the open overlays, top first,
before the pane trap runs (`components/shared/overlayFocus.ts`):

- A **dialog** (a backdrop, or `aria-modal`) contains Tab. Focus outside its
  panel moves in, and Tab wraps at the panel's two ends. Focus on an unlisted
  element inside, such as a clicked dialog body, steps to its listed neighbour.
- An **anchored popover** wraps Tab while focus is inside it. A Tab that starts
  outside it closes it, and the walk continues to the overlay below, then the
  pane trap.

`<Overlay>` also moves focus into a dialog on open when nothing inside took it.
On close it returns focus to the opener, or to the anchor when a click left no
focus (WebKit), under two conditions. Focus went down with the panel. And the
overlay's action left the focused-pane marker in the opener's pane, or the
opener sits outside every pane, such as a header button.

## Rationale

The two shapes follow what the reader is doing. A dialog is modal: nothing
behind it may be answered, so the keyboard stays with it. A popover hangs off a
control the reader is still using: the combobox input they are typing in, or
the button that opened a menu. A Tab from that control means "move on", so the
popover gets out of the way. A Tab from inside a popover means the reader is
working through it, so it wraps.

One place, not one per overlay. The dismiss, swallow and Escape halves of the
overlay contract already live in `<Overlay>` for the same reason: a contract
each overlay wires itself is one that some overlay forgets.

The restore defers to the marker because an overlay action can navigate. A menu
item that opens a file has moved the reader to the content pane. Pulling focus
back to the menu's button would split focus from the marker again, which is the
bug class the audit was about.

## Consequences

- The five hand-rolled dialog traps and `dialogFocusTrap.ts` are gone.
- A popover with form controls, reached by pointer with focus left on its
  anchor, closes on Tab. The reader reaches its controls by clicking, or by a
  keyboard path that opens it with focus inside, as the ⋯ menu does.
- Focus-in and restore run on keyboard devices only (`hasHoverPointer`), since
  focusing a field on a phone raises its keyboard.

## Alternatives considered

- **Contain Tab in every overlay.** A free-text dropdown's menu is an overlay
  whose control sits outside its panel. Containing Tab would pull focus from the
  input into the menu, which is the opposite of moving on.
- **Close every overlay on Tab.** A dialog that closes on Tab loses the reader's
  half-filled form, and an aria-modal panel must keep focus by definition.
- **Keep per-dialog traps and add the missing ones.** That is how the gap
  opened: each new dialog had to remember, and most did not.
- **Use the `inert` attribute on the shell.** It would stop focus as well as the
  pointer. But the anchor must stay live and a descendant cannot override an
  inert ancestor. That is why the shell uses `pointer-events` in the first place
  (`.claude/rules/frontend.md` § Modals & Popovers, item 7).
