import type { ComponentChildren, JSX } from 'preact';
import { useState, useRef, useEffect } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition, pointAnchor, type AnchorAlign, type AnchorBox, type AnchorPosition, type ViewportPoint } from '../../hooks/useAnchoredPopover';
import { useObjectMenu } from './useObjectMenu';
import { MoreIcon } from './icons';
import type { ChunkGate } from '../../utils/lazyComponent';
import { usePendingOpen } from '../../hooks/usePendingOpen';

export function overflowMenuPanelStyle(pos: Pick<AnchorPosition, 'top' | 'left' | 'maxHeight'> | null): JSX.CSSProperties {
  return pos
    ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px`, '--anchor-room': `${pos.maxHeight}px` }
    : { visibility: 'hidden' };
}

const MENU_ITEM_ROLES = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]';

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

export interface OverflowMenuContext {
  run: (fn: () => void) => (e: MouseEvent) => void;
  anchor: HTMLElement | null;
}

export type OverflowMenuOpener = (anchor: HTMLElement, at?: ViewportPoint) => void;

export interface HostOpener {
  ref: { current: OverflowMenuOpener | null };
  trigger: boolean;
  align?: AnchorAlign;
}

export interface TriggerFace {
  class: string;
  children: ComponentChildren;
  label?: string;
}

type MenuOpening =
  | { hostOpener?: HostOpener; face?: never }
  | { face: TriggerFace; hostOpener?: never };

interface Placement {
  anchor: HTMLElement;
  box: AnchorBox;
  align: AnchorAlign;
}

export function OverflowMenu({ ariaLabel, stopPropagation, extraClass, triggerAttrs, onOpen, tabIndex, hostOpener, face, items, itemsChunk, protectedSurface }: MenuOpening & {
  ariaLabel: string;
  stopPropagation?: boolean;
  extraClass?: string;
  triggerAttrs?: Record<string, string>;
  onOpen?: () => void;
  tabIndex?: number;
  items: (ctx: OverflowMenuContext) => ComponentChildren;
  itemsChunk?: ChunkGate;
  protectedSurface?: boolean;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const drawsDots = !face && (hostOpener?.trigger ?? true);
  const drawsTrigger = drawsDots || !!face;
  const [menu, setMenu] = useState<Placement | null>(null);
  const [openedViaKeyboard, setOpenedViaKeyboard] = useState(false);
  const lastFocusedRef = useRef<HTMLElement | null>(null);
  const open = menu !== null;
  const pos = useAnchoredPosition(menu?.box ?? null, menuRef, undefined, menu?.align);

  const pendingOpen = usePendingOpen();
  const openWhenReady = (place: () => void, from: Element) => {
    if (itemsChunk) pendingOpen.open(itemsChunk, place, from);
    else place();
  };

  const placeAt = (anchor: HTMLElement): Placement =>
    ({ anchor, box: anchor, align: drawsDots && anchor === triggerRef.current ? 'end' : hostOpener?.align ?? 'start' });

  const close = () => {
    setMenu(null);
    if (openedViaKeyboard) {
      const prev = lastFocusedRef.current;
      if (prev && document.body.contains(prev)) prev.focus();
    }
  };
  const toggle = (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    if (open) { close(); return; }
    if (pendingOpen.takeBack()) return;
    const trigger = triggerRef.current;
    if (!trigger) return;
    const keyboard = e.detail === 0;
    const focused = document.activeElement as HTMLElement | null;
    openWhenReady(() => {
      onOpen?.();
      setOpenedViaKeyboard(keyboard);
      lastFocusedRef.current = keyboard ? focused : null;
      setMenu(placeAt(trigger));
    }, trigger);
  };
  const run = (fn: () => void) => (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    close();
    fn();
  };

  const openFromGesture = (el: HTMLElement, at?: ViewportPoint) => {
    openWhenReady(() => {
      onOpen?.();
      setOpenedViaKeyboard(false);
      lastFocusedRef.current = null;
      setMenu(at ? { anchor: el, box: pointAnchor(at), align: 'start' } : placeAt(el));
    }, el);
  };
  const hostRef = hostOpener?.ref;
  if (hostRef) hostRef.current = openFromGesture;
  useEffect(() => () => { if (hostRef) hostRef.current = null; }, [hostRef]);

  const handleMenuKeyDown = (e: KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const panel = menuRef.current;
    if (!panel) return;
    const menuItems = Array.from(panel.querySelectorAll<HTMLElement>(MENU_ITEM_ROLES));
    if (menuItems.length === 0) return;
    e.preventDefault();
    const current = menuItems.indexOf(document.activeElement as HTMLElement);
    menuItems[nextMenuIndex(current, menuItems.length, e.key)]?.focus();
  };

  useEffect(() => {
    if (!open || !openedViaKeyboard || !pos) return;
    const panel = menuRef.current;
    if (!panel || panel.contains(document.activeElement)) return;
    panel.querySelector<HTMLElement>(MENU_ITEM_ROLES)?.focus();
  }, [open, openedViaKeyboard, pos]);

  const overlayAnchor = drawsTrigger ? triggerRef.current : null;

  return (
    <>
      {face && (
        <FaceTrigger face={face} attrs={triggerAttrs} open={open} triggerRef={triggerRef} onTap={toggle} onGesture={openFromGesture} />
      )}
      {drawsDots && (
        <button
          ref={triggerRef}
          type="button"
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
        panelClass={`surface-box thread-overflow-menu${protectedSurface ? ' protected-surface' : ''}`}
        panelRole="menu"
        panelRef={menuRef}
        panelProps={{ onKeyDown: handleMenuKeyDown }}
        panelStyle={overflowMenuPanelStyle(pos)}
      >
        {menu && items({ run, anchor: menu.anchor })}
      </Overlay>
    </>
  );
}

function FaceTrigger({ face, attrs, open, triggerRef, onTap, onGesture }: {
  face: TriggerFace;
  attrs?: Record<string, string>;
  open: boolean;
  triggerRef: { current: HTMLButtonElement | null };
  onTap: (e: MouseEvent) => void;
  onGesture: OverflowMenuOpener;
}) {
  const menu = useObjectMenu({ onTap });
  menu.opener.current = onGesture;
  return (
    <button
      ref={triggerRef}
      type="button"
      {...attrs}
      class={face.class}
      aria-label={face.label}
      data-tooltip={face.label}
      aria-haspopup="menu"
      aria-expanded={open}
      {...menu.press}
      onClick={menu.onClick}
    >
      {face.children}
    </button>
  );
}
