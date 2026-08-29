import type { ComponentChildren } from 'preact';
import { useState, useRef, useEffect } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { MoreIcon, InfoIcon } from './icons';
import type { TooltipRow } from '../drawer/threadRowInfo';

/** Roving-focus index for a ⋯ menu's ↑/↓/Home/End keys. `current` is the
 *  focused item's index (-1 when focus isn't yet on an item); ↑/↓ wrap at both
 *  ends. Pure — exported for unit testing. */
export function nextMenuIndex(current: number, count: number, key: string): number {
  if (count <= 0) return -1;
  switch (key) {
    case 'Home': return 0;
    case 'End': return count - 1;
    case 'ArrowDown': return current < 0 ? 0 : (current + 1) % count;
    case 'ArrowUp': return current < 0 ? count - 1 : (current - 1 + count) % count;
    default: return current;
  }
}

/** Context handed to an overflow menu's `items` renderer. `run` wraps an action
 *  so it closes the menu (and stops the host row's click) before firing;
 *  `openedViaKeyboard` lets an item show only on keyboard-open (see the Pin
 *  item in ThreadOverflowMenu). */
export interface OverflowMenuContext {
  open: boolean;
  openedViaKeyboard: boolean;
  run: (fn: () => void) => (e: MouseEvent) => void;
}

/** How a host opens a menu that renders no ⋯ trigger. The element passed is what
 *  the popovers position against. */
export type OverflowMenuOpener = (anchor: HTMLElement) => void;

/** Generic ⋯ overflow menu: a trigger button, an anchored menu popover, and an
 *  optional secondary Info popover — the whole dismiss/Escape/inert contract via
 *  the central <Overlay>, `portal`ed + `position: fixed` so it escapes the
 *  drawer's scroll/overflow clipping and any transformed header ancestor. The
 *  shared shell behind ThreadOverflowMenu (started threads) and DraftOverflowMenu
 *  (compose drafts); each supplies its own `items` and `infoRows`.
 *
 *  **`openRef` hands opening to the host, and the mobile drawer row is why.**
 *  There the ⋯ is not drawn and a long press on the row opens the menu instead:
 *  a 31x27px trigger against the pane's right edge is the hardest place on a
 *  phone to hit. One prop rather than a hide-the-trigger flag beside it. A menu
 *  with neither a trigger nor a host opener would be unopenable, and that state
 *  is now unrepresentable.
 *
 *  **Open mode (keyboard vs pointer) shapes the menu.** A real pointer click
 *  reports `e.detail >= 1`; a keyboard activation (Enter/Space) and a synthetic
 *  `trigger.click()` both report `e.detail === 0`. A keyboard-open moves focus
 *  into the menu so ↑/↓/Home/End rove the items and Enter runs the highlighted
 *  action, and hands focus back to the opener on close so list-nav resumes; a
 *  pointer-open leaves focus where it was. Items may branch on
 *  `ctx.openedViaKeyboard` to expose keyboard-only actions.
 *
 *  **Signal gating.** `items` is invoked ONLY while the menu is open and
 *  `infoRows` ONLY while a popover is open, so a closed menu subscribes to no
 *  hot signals — preserving the drawer's per-row render budget across the many
 *  rows that each mount one of these.
 *
 *  `stopPropagation` guards a host whose container has its own click handler
 *  (the drawer row's focus-thread `onClick`) — toggling the menu or running an
 *  item must not also fire it.
 */
export function OverflowMenu({ ariaLabel, stopPropagation, extraClass, tabIndex, openRef, items, infoRows }: {
  ariaLabel: string;
  stopPropagation?: boolean;
  extraClass?: string;
  /** `-1` removes the ⋯ trigger from the Tab order (the drawer row's mouse-only
   *  use — the drawer is a single tab stop, and the menu is opened via the
   *  "Open thread actions" shortcut). Default undefined → natively tabbable
   *  (the thread-title headers). Ignored in host-opened mode: there is no
   *  trigger to order. */
  tabIndex?: number;
  /** Set it and the host owns opening: no ⋯ is rendered, and this menu writes
   *  its opener here for the host's gesture to call. See the class comment. */
  openRef?: { current: OverflowMenuOpener | null };
  /** Custom items, rendered above the auto-appended Info row. Invoked only while
   *  the menu is open. */
  items: (ctx: OverflowMenuContext) => ComponentChildren;
  /** Structured Info rows; a null/empty result omits the Info item + popover
   *  (e.g. an unhydrated search hit with no live meta). Invoked only while a
   *  popover is open. */
  infoRows?: () => TooltipRow[] | null;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const infoRef = useRef<HTMLDivElement>(null);
  // The element the host last opened against, so the Info popover lands on the
  // same one the menu did. Null whenever a ⋯ trigger is doing the opening.
  const hostAnchorRef = useRef<HTMLElement | null>(null);
  // Anchor element when open, null when closed — `useAnchoredPosition` reacts to
  // anchor changes via its effect deps, so no separate `open` flag is needed.
  // The menu and the Info popover each carry their own anchor; they're mutually
  // exclusive (opening one closes the other).
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [infoAnchor, setInfoAnchor] = useState<HTMLElement | null>(null);
  // Whether the LAST open was keyboard-driven (Enter/Space on the trigger or the
  // shortcut's synthetic `trigger.click()`, both `e.detail === 0`). Drives the
  // focus-into-menu on open and the focus-restore on close.
  const [openedViaKeyboard, setOpenedViaKeyboard] = useState(false);
  // Element focused at keyboard-open time (the drawer tree container, or the
  // header trigger), restored on close so list-nav resumes.
  const lastFocusedRef = useRef<HTMLElement | null>(null);
  const open = anchor !== null;
  const infoOpen = infoAnchor !== null;
  // Right-align both popovers to the ⋯ trigger: the trigger sits at the far right
  // of a drawer row / thread-title header, so left-start placement would push the
  // wide panel off-screen and the viewport clamp would strand it near the left
  // edge (the "all the way to the left" report on narrow mobile viewports).
  //
  // Host-opened mode inverts that, because the anchor does too: a whole drawer
  // row, not a button at its end. Right-aligning to it puts the panel back in
  // the corner the long press exists to leave. So it aligns to the row's
  // leading edge instead.
  const align = openRef ? 'start' : 'end';
  const pos = useAnchoredPosition(anchor, menuRef, undefined, align);
  const infoPos = useAnchoredPosition(infoAnchor, infoRef, undefined, align);

  const close = () => {
    setAnchor(null);
    // A keyboard-opened menu owns DOM focus (an item inside the portal); hand it
    // back to the opener so the drawer's ↑/↓/Enter list-nav (or the header) is
    // live again after an action instead of focus falling to <body>. A
    // pointer-opened menu never moved focus, so there's nothing to restore.
    if (openedViaKeyboard) {
      const prev = lastFocusedRef.current;
      if (prev && document.body.contains(prev)) prev.focus();
    }
  };
  const closeInfo = () => setInfoAnchor(null);
  const toggle = (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    closeInfo();
    if (open) { close(); return; }
    // detail === 0 ⇒ keyboard activation or the shortcut's synthetic click.
    const keyboard = e.detail === 0;
    setOpenedViaKeyboard(keyboard);
    lastFocusedRef.current = keyboard ? (document.activeElement as HTMLElement | null) : null;
    setAnchor(triggerRef.current);
  };
  const run = (fn: () => void) => (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    close();
    fn();
  };

  // Host-opened mode's entry point, published for the host's own gesture. It
  // only ever OPENS: the host cannot call it while the menu is up, because an
  // open overlay makes the shell behind it inert and the gesture never starts.
  // Always a pointer-open, since a gesture is what invokes it.
  //
  // Assigned during render rather than in an effect, so it is live from the
  // first frame. Cleared on unmount, so a row scrolled out of the list cannot
  // be opened through a stale handle.
  if (openRef) {
    openRef.current = (el: HTMLElement) => {
      closeInfo();
      hostAnchorRef.current = el;
      setOpenedViaKeyboard(false);
      lastFocusedRef.current = null;
      setAnchor(el);
    };
  }
  useEffect(() => () => { if (openRef) openRef.current = null; }, [openRef]);

  // ↑/↓/Home/End rove focus across the menu items (the Overlay owns Escape;
  // Enter/Space activate the focused <button> natively → its onClick).
  const handleMenuKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const panel = menuRef.current;
    if (!panel) return;
    const menuItems = Array.from(panel.querySelectorAll<HTMLElement>('[role="menuitem"]'));
    if (menuItems.length === 0) return;
    e.preventDefault();
    const current = menuItems.indexOf(document.activeElement as HTMLElement);
    menuItems[nextMenuIndex(current, menuItems.length, e.key)]?.focus();
  };

  // Keyboard-open: once the panel is positioned (it's `visibility: hidden` — and
  // unfocusable — until `pos` lands) move focus to the first item so ↑/↓ drive
  // the menu and Enter runs the highlighted action. Guard on the panel not
  // already owning focus so a reposition (scroll/resize recompute) never yanks
  // focus back to the top.
  useEffect(() => {
    if (!open || !openedViaKeyboard || !pos) return;
    const panel = menuRef.current;
    if (!panel || panel.contains(document.activeElement)) return;
    panel.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [open, openedViaKeyboard, pos]);

  // Info rows drive both the Info menu item's presence and the Info popover body.
  // Read ONLY while a popover is open so a closed menu subscribes to no signals.
  const rows = (open || infoOpen) && infoRows ? infoRows() : null;

  // The Overlay's anchor is the element that re-activates the overlay through
  // its OWN handler, exempt from the outside-pointerdown dismiss. That is the ⋯
  // trigger, and host-opened mode has none. The row is not a toggle: its click
  // focuses the thread, so exempting it would let one tap both dismiss the menu
  // and navigate. The re-open race the exemption normally guards cannot happen
  // here, because a short tap never opens this menu.
  const overlayAnchor = openRef ? null : triggerRef.current;

  return (
    <>
      {!openRef && (
        <button
          ref={triggerRef}
          type="button"
          tabIndex={tabIndex}
          class={`icon-btn header-icon${extraClass ? ` ${extraClass}` : ''}`}
          onClick={toggle}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={ariaLabel}
          data-tooltip="More actions"
        >
          <MoreIcon />
        </button>
      )}
      <Overlay
        open={open}
        onClose={close}
        anchor={overlayAnchor}
        backdrop={false}
        portal
        panelClass="thread-overflow-menu"
        panelRole="menu"
        panelRef={menuRef}
        panelProps={{ onKeyDown: handleMenuKeyDown }}
        panelStyle={pos
          ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px` }
          : { visibility: 'hidden' }}
      >
        {open && items({ open, openedViaKeyboard, run })}
        {rows && (
          <>
            <div class="thread-overflow-divider" role="separator" />
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => setInfoAnchor(triggerRef.current ?? hostAnchorRef.current))}>
              <InfoIcon />
              Info
            </button>
          </>
        )}
      </Overlay>
      <Overlay
        open={infoOpen}
        onClose={closeInfo}
        anchor={overlayAnchor}
        backdrop={false}
        portal
        panelClass="thread-info-popover"
        panelRef={infoRef}
        panelStyle={infoPos
          ? { position: 'fixed', top: `${infoPos.top}px`, left: `${infoPos.left}px` }
          : { visibility: 'hidden' }}
      >
        {rows && (
          <div class="thread-info-rows">
            {rows.map((r) => (
              <div class="thread-info-row" key={r.label}>
                <span class="thread-info-label">{r.label}</span>
                <span class="thread-info-value">
                  {r.tone && <span class={`thread-info-dot thread-info-dot-${r.tone}`} />}
                  {r.value}
                </span>
              </div>
            ))}
          </div>
        )}
      </Overlay>
    </>
  );
}
