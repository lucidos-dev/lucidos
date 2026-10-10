import type { ComponentChildren } from 'preact';
import { useState, useRef, useEffect } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition, pointAnchor, type AnchorAlign, type AnchorBox, type ViewportPoint } from '../../hooks/useAnchoredPopover';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import { MoreIcon } from './icons';

/** What counts as a row for roving focus and for the keyboard-open focus. */
const MENU_ITEM_ROLES = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]';

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
 *  so it closes the menu (and stops the host row's click) before firing. */
export interface OverflowMenuContext {
  run: (fn: () => void) => (e: MouseEvent) => void;
  /** What the menu is anchored to: the ⋯ trigger, or the host's element when
   *  the host opened it. An item that OPENS a popover of its own needs it, since
   *  the row it was clicked on unmounts as the menu closes. */
  anchor: HTMLElement | null;
}

/** How a host opens the menu from its own gesture. `anchor` is the element the
 *  menu belongs to. `at` opens it at the pointer instead of below `anchor`. */
export type OverflowMenuOpener = (anchor: HTMLElement, at?: ViewportPoint) => void;

/** Hands opening to a host. `trigger` says whether the ⋯ is drawn as well.
 *  `align` places a menu the host opened against its anchor, and defaults to
 *  the anchor's leading edge. */
export interface HostOpener {
  ref: { current: OverflowMenuOpener | null };
  trigger: boolean;
  align?: AnchorAlign;
}

/** The host's own content drawn as the menu button in place of the ⋯, such as
 *  a thread title. A tap toggles the menu; a hold or a right-click opens it. */
export interface TriggerFace {
  class: string;
  children: ComponentChildren;
}

/** One way in, at most: a face cannot be combined with a host opener, whose
 *  `trigger: false` would otherwise contradict it. */
type MenuOpening =
  | { hostOpener?: HostOpener; face?: never }
  | { face: TriggerFace; hostOpener?: never };

/** An open popover: the element it belongs to, and the box it is placed
 *  against. The two differ only for a menu opened at the pointer. */
interface Placement {
  anchor: HTMLElement;
  box: AnchorBox;
  align: AnchorAlign;
}

/** Generic ⋯ overflow menu: a trigger button and an anchored menu popover, with
 *  the whole dismiss/Escape/inert contract via the central <Overlay>, `portal`ed
 *  + `position: fixed` so it escapes the drawer's scroll/overflow clipping and
 *  any transformed header ancestor. The shared shell behind ThreadOverflowMenu
 *  (started threads) and DraftOverflowMenu (compose drafts); each supplies its
 *  own `items`.
 *
 *  **`hostOpener` lets the host open the same menu from a gesture.** A desktop
 *  drawer row's right-click opens it at the pointer, beside a drawn ⋯ (ADR
 *  0285). A mobile row draws no ⋯ and a long press opens it instead: a 31x27px
 *  trigger against the pane's right edge is the hardest place on a phone to
 *  hit. The ⋯ can only be hidden through `hostOpener` or a `face`, so a menu
 *  nothing can open is unrepresentable.
 *
 *  **`face` draws the host's content as the trigger.** The thread title is its
 *  own menu button on both title rows. It is a toggle, so it stays the
 *  overlay's anchor, and the menu opens against it, aligned to its leading edge.
 *
 *  **Open mode (keyboard vs pointer) shapes the menu.** A real pointer click
 *  reports `e.detail >= 1`; a keyboard activation (Enter/Space) and a synthetic
 *  `trigger.click()` both report `e.detail === 0`. A keyboard-open moves focus
 *  into the menu so ↑/↓/Home/End rove the items and Enter runs the highlighted
 *  action, and hands focus back to the opener on close so list-nav resumes; a
 *  pointer-open leaves focus where it was.
 *
 *  **Signal gating.** `items` is invoked ONLY while the menu is open. A closed
 *  menu then subscribes to no hot signals. That keeps the drawer's per-row
 *  render budget across the many rows that each mount one of these.
 *
 *  `stopPropagation` guards a host whose container has its own click handler
 *  (the drawer row's focus-thread `onClick`) — toggling the menu or running an
 *  item must not also fire it.
 */
export function OverflowMenu({ ariaLabel, stopPropagation, extraClass, triggerAttrs, onOpen, tabIndex, hostOpener, face, items }: MenuOpening & {
  ariaLabel: string;
  stopPropagation?: boolean;
  extraClass?: string;
  /** Extra attributes for the ⋯ trigger. A host whose row is MEASURED marks its
   *  members with one, and the trigger is a member like any other. Ignored
   *  when no trigger is drawn. */
  triggerAttrs?: Record<string, string>;
  /** Run as the menu opens, before anything else.
   *
   *  For a host whose items open popovers of their OWN against this trigger.
   *  The trigger is then that popover's anchor, and an anchor is exempt from
   *  the outside-click dismiss. So re-pressing it would stack a menu over a
   *  panel that will not go. Retire them here instead. */
  onOpen?: () => void;
  /** `-1` removes the ⋯ trigger from the Tab order (the drawer row's mouse-only
   *  use — the drawer is a single tab stop, and the menu is opened via the
   *  "Open thread actions" shortcut). Default undefined → natively tabbable.
   *  Ignored when no trigger is drawn. */
  tabIndex?: number;
  /** The menu's items. Invoked only while the menu is open. */
  items: (ctx: OverflowMenuContext) => ComponentChildren;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const drawsDots = !face && (hostOpener?.trigger ?? true);
  const drawsTrigger = drawsDots || !!face;
  // Null when closed. `useAnchoredPosition` reacts to `box` changes via its
  // effect deps, so no separate `open` flag is needed.
  const [menu, setMenu] = useState<Placement | null>(null);
  // Whether the LAST open was keyboard-driven (Enter/Space on the trigger or the
  // shortcut's synthetic `trigger.click()`, both `e.detail === 0`). Drives the
  // focus-into-menu on open and the focus-restore on close.
  const [openedViaKeyboard, setOpenedViaKeyboard] = useState(false);
  // Element focused at keyboard-open time (the drawer tree container, or the
  // header trigger), restored on close so list-nav resumes.
  const lastFocusedRef = useRef<HTMLElement | null>(null);
  const open = menu !== null;
  const pos = useAnchoredPosition(menu?.box ?? null, menuRef, undefined, menu?.align);

  // Right-align to the ⋯ trigger. It sits at the far right of its row. A
  // left-start panel would run off-screen, and the clamp would then strand it
  // near the left edge.
  //
  // A host's anchor is a whole row, not a button at its end. Right-aligning to
  // it puts the panel back in the corner the long press exists to leave, so it
  // aligns to the row's leading edge instead, unless the host asks otherwise.
  const placeAt = (anchor: HTMLElement): Placement =>
    ({ anchor, box: anchor, align: drawsDots && anchor === triggerRef.current ? 'end' : hostOpener?.align ?? 'start' });

  const close = () => {
    setMenu(null);
    // A keyboard-opened menu owns DOM focus (an item inside the portal); hand it
    // back to the opener so the drawer's ↑/↓/Enter list-nav (or the header) is
    // live again after an action instead of focus falling to <body>. A
    // pointer-opened menu never moved focus, so there's nothing to restore.
    if (openedViaKeyboard) {
      const prev = lastFocusedRef.current;
      if (prev && document.body.contains(prev)) prev.focus();
    }
  };
  const toggle = (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    if (open) { close(); return; }
    const trigger = triggerRef.current;
    if (!trigger) return;
    onOpen?.();
    // detail === 0 ⇒ keyboard activation or the shortcut's synthetic click.
    const keyboard = e.detail === 0;
    setOpenedViaKeyboard(keyboard);
    lastFocusedRef.current = keyboard ? (document.activeElement as HTMLElement | null) : null;
    setMenu(placeAt(trigger));
  };
  const run = (fn: () => void) => (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    close();
    fn();
  };

  // The host's entry point, published for its own gesture. It only ever OPENS:
  // the host cannot call it while the menu is up, because an open overlay makes
  // the shell behind it inert and the gesture never starts. Always a
  // pointer-open, since a gesture is what invokes it.
  //
  // Assigned during render rather than in an effect, so it is live from the
  // first frame. Cleared on unmount, so a row scrolled out of the list cannot
  // be opened through a stale handle.
  const openFromGesture = (el: HTMLElement, at?: ViewportPoint) => {
    onOpen?.();
    setOpenedViaKeyboard(false);
    lastFocusedRef.current = null;
    setMenu(at ? { anchor: el, box: pointAnchor(at), align: 'start' } : placeAt(el));
  };
  const hostRef = hostOpener?.ref;
  if (hostRef) hostRef.current = openFromGesture;
  useEffect(() => () => { if (hostRef) hostRef.current = null; }, [hostRef]);

  // ↑/↓/Home/End rove focus across the menu items (the Overlay owns Escape;
  // Enter/Space activate the focused <button> natively → its onClick).
  const handleMenuKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const panel = menuRef.current;
    if (!panel) return;
    // Every menu role, not just the plain one. A menu whose rows record a
    // CHOICE marks them `menuitemradio`. That is the only role able to carry
    // `aria-checked`, and such rows have to rove like any other.
    const menuItems = Array.from(panel.querySelectorAll<HTMLElement>(MENU_ITEM_ROLES));
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
    panel.querySelector<HTMLElement>(MENU_ITEM_ROLES)?.focus();
  }, [open, openedViaKeyboard, pos]);

  // The Overlay's anchor is the element that re-activates the overlay through
  // its OWN handler, exempt from the outside-pointerdown dismiss. That is the ⋯
  // or the face, whichever way the menu opened, so pressing it closes the menu. A
  // host's row is never the anchor: its click focuses the thread, so exempting
  // it would let one tap both dismiss the menu and navigate. The re-open race
  // the exemption guards cannot happen there, because a tap never opens it.
  const overlayAnchor = drawsTrigger ? triggerRef.current : null;

  return (
    <>
      {face && (
        <FaceTrigger face={face} open={open} triggerRef={triggerRef} onTap={toggle} onGesture={openFromGesture} />
      )}
      {drawsDots && (
        <button
          ref={triggerRef}
          type="button"
          // The spread leads, so the trigger's own class and tab order always
          // win over a host's attributes rather than the other way round.
          {...triggerAttrs}
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
        panelClass="surface-box thread-overflow-menu"
        panelRole="menu"
        panelRef={menuRef}
        panelProps={{ onKeyDown: handleMenuKeyDown }}
        panelStyle={pos
          ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px` }
          : { visibility: 'hidden' }}
      >
        {menu && items({ run, anchor: menu.anchor })}
      </Overlay>
    </>
  );
}

/** The host's content as the menu button. A still hold opens the menu and its
 *  lift is swallowed, a tap toggles it, and a right-click opens it at the
 *  pointer (ADR 0285). A phone places it against the button instead, since a
 *  finger covers the point. */
function FaceTrigger({ face, open, triggerRef, onTap, onGesture }: {
  face: TriggerFace;
  open: boolean;
  triggerRef: { current: HTMLButtonElement | null };
  onTap: (e: MouseEvent) => void;
  onGesture: OverflowMenuOpener;
}) {
  const press = useLongPress(
    (el, at) => onGesture(el, viewportIsMobile.value ? undefined : at),
    onTap,
  );
  return (
    <button
      ref={triggerRef}
      type="button"
      class={face.class}
      aria-haspopup="menu"
      aria-expanded={open}
      onPointerDown={press.onPointerDown}
      onPointerMove={press.onPointerMove}
      onPointerUp={press.onPointerUp}
      onPointerLeave={press.onPointerLeave}
      onPointerCancel={press.onPointerCancel}
      onContextMenu={(e) => { if (!e.altKey) press.onContextMenu(e); }}
      onClick={press.onClick}
    >
      {face.children}
    </button>
  );
}
