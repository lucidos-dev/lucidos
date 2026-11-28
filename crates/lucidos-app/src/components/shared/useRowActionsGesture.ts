import { useRef } from 'preact/hooks';
import { useLongPress, type LongPressHandlers } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import type { HostOpener, OverflowMenuOpener } from './OverflowMenu';

export interface RowGestureHandlers {
  onPointerDown?: (e: PointerEvent) => void;
  onPointerMove?: (e: PointerEvent) => void;
  onPointerUp?: (e: PointerEvent) => void;
  onPointerLeave?: (e: PointerEvent) => void;
  onPointerCancel?: (e: PointerEvent) => void;
  onContextMenu?: (e: MouseEvent) => void;
  onClick?: (e: MouseEvent) => void;
}

export interface RowActionsGesture {
  hostOpener: HostOpener;
  handlers: RowGestureHandlers;
}

function startsOnControl(e: { target: EventTarget | null }): boolean {
  return !!(e.target as Element | null)?.closest?.('button, label');
}

export function rowGestureHandlers({ mobile, enabled, press, onPress }: {
  mobile: boolean;
  enabled: boolean;
  press: LongPressHandlers;
  onPress?: () => void;
}): RowGestureHandlers {
  if (!enabled) return {};
  const onContextMenu = (e: MouseEvent) => { if (!startsOnControl(e)) press.onContextMenu(e); };

  if (!mobile) {
    return {
      onPointerDown: () => {
        onPress?.();
        press.cancel();
      },
      onContextMenu: (e) => { if (!e.altKey) onContextMenu(e); },
      onClick: press.onClick,
    };
  }

  return {
    onPointerDown: (e) => {
      onPress?.();
      if (startsOnControl(e)) return;
      press.onPointerDown(e);
    },
    onPointerMove: press.onPointerMove,
    onPointerUp: press.onPointerUp,
    onPointerLeave: press.onPointerLeave,
    onPointerCancel: press.onPointerCancel,
    onContextMenu,
    onClick: press.onClick,
  };
}

export function useRowActionsGesture({ onTap, onPress, enabled }: {
  onTap?: () => void;
  onPress?: () => void;
  enabled: boolean;
}): RowActionsGesture {
  const openRef = useRef<OverflowMenuOpener | null>(null);
  const mobile = viewportIsMobile.value;
  const press = useLongPress(
    (row, at) => openRef.current?.(row, viewportIsMobile.value ? undefined : at),
    () => onTap?.(),
  );
  return {
    hostOpener: { ref: openRef, trigger: !(mobile && enabled) },
    handlers: rowGestureHandlers({ mobile, enabled, press, onPress }),
  };
}
