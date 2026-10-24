import { useRef } from 'preact/hooks';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import type { OverflowMenuOpener } from './OverflowMenu';

/** The press handlers an object spreads. Typed on the DOM events, so a host
 *  can spread them onto any element. */
export interface ObjectPressHandlers {
  onPointerDown: (e: PointerEvent) => void;
  onPointerMove: (e: PointerEvent) => void;
  onPointerUp: (e: PointerEvent) => void;
  onPointerLeave: (e: PointerEvent) => void;
  onPointerCancel: (e: PointerEvent) => void;
  onContextMenu: (e: MouseEvent) => void;
}

export interface ObjectMenu {
  /** The object's `OverflowMenu` publishes its opener here: pass it as the
   *  `hostOpener` ref. */
  opener: { current: OverflowMenuOpener | null };
  /** Opens the menu against `el`, or at `at` on a desktop viewport. */
  open: OverflowMenuOpener;
  press: ObjectPressHandlers;
  /** Runs the object's tap, or swallows the click a hold paired. */
  onClick: (e: MouseEvent) => void;
}

/** An object with actions (ADR 0285): a right-click or a long press on it
 *  opens its menu. A phone places the menu against the object, since a finger
 *  covers the point. Option+right-click is left to the native menu.
 *
 *  `onTap` is the object's own tap. `anchor` names the part of a large object
 *  a menu with no point opens against, such as a widget's bar. */
export function useObjectMenu({ onTap = () => {}, anchor = (el) => el }: {
  onTap?: (e: MouseEvent) => void;
  anchor?: (el: HTMLElement) => HTMLElement;
} = {}): ObjectMenu {
  const opener = useRef<OverflowMenuOpener | null>(null);
  const open: OverflowMenuOpener = (el, at) => {
    const point = viewportIsMobile.value ? undefined : at;
    opener.current?.(point ? el : anchor(el), point);
  };
  const press = useLongPress(open, onTap);
  return {
    opener,
    open,
    press: {
      onPointerDown: press.onPointerDown,
      onPointerMove: press.onPointerMove,
      onPointerUp: press.onPointerUp,
      onPointerLeave: press.onPointerLeave,
      onPointerCancel: press.onPointerCancel,
      onContextMenu: (e) => { if (!e.altKey) press.onContextMenu(e); },
    },
    onClick: press.onClick,
  };
}
