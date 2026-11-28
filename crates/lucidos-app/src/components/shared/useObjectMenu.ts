import { useRef } from 'preact/hooks';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import type { OverflowMenuOpener } from './OverflowMenu';

export interface ObjectPressHandlers {
  onPointerDown: (e: PointerEvent) => void;
  onPointerMove: (e: PointerEvent) => void;
  onPointerUp: (e: PointerEvent) => void;
  onPointerLeave: (e: PointerEvent) => void;
  onPointerCancel: (e: PointerEvent) => void;
  onContextMenu: (e: MouseEvent) => void;
}

export interface ObjectMenu {
  opener: { current: OverflowMenuOpener | null };
  open: OverflowMenuOpener;
  press: ObjectPressHandlers;
  onClick: (e: MouseEvent) => void;
}

export function isMenuKey(e: KeyboardEvent): boolean {
  return e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10');
}

export function useObjectMenu({ onTap = () => {} }: {
  onTap?: (e: MouseEvent) => void;
} = {}): ObjectMenu {
  const opener = useRef<OverflowMenuOpener | null>(null);
  const open: OverflowMenuOpener = (el, at) => {
    opener.current?.(el, viewportIsMobile.value ? undefined : at);
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
