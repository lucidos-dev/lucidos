import { useRef } from 'preact/hooks';
import { useLongPress, type LongPressHandlers } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import type { HostOpener, OverflowMenuOpener } from '../shared/OverflowMenu';

/** The pointer handlers a drawer row spreads. Typed on the DOM events, not
 *  Preact's targeted ones, so the row can spread this onto its `<div>` while
 *  the hook stays testable without JSX. */
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
  /** Handed to the row's overflow menu. `trigger` is false on mobile, where
   *  the hold replaces the ⋯ as the way in. */
  hostOpener: HostOpener;
  handlers: RowGestureHandlers;
}

/** A press that began on an inline control belongs to that control. The pin and
 *  the family disclosure both live inside the row, and both would otherwise
 *  arm the row's hold underneath them: holding the pin would pin the thread AND
 *  open the menu. */
function startsOnControl(e: { target: EventTarget | null }): boolean {
  return !!(e.target as Element | null)?.closest?.('button');
}

/** The row's handlers for one layout, given the row's gesture machine. Pure, so
 *  both layouts are testable without a renderer.
 *
 *  **Desktop** keeps the ⋯ and adds a right-click that opens the same menu at
 *  the pointer (ADR 0285). There is no hold. `press` still owns the click, so
 *  a ctrl-click's paired click cannot also open the thread. A fresh press
 *  clears that arm, so the next plain click does. Option+right-click is left
 *  to the native menu, which ADR 0285 promises everywhere.
 *
 *  **Mobile** drops the ⋯ and makes the whole row the target: the ⋯ is a
 *  31x27px box against the pane's right edge, the hardest place on a phone to
 *  reach. A scroll never opens the menu, since `useLongPress` cancels the hold
 *  once the pointer travels 10px. Android fires `contextmenu` for a long press,
 *  and would draw the browser's own menu over ours. iOS does not, the callout
 *  being suppressed on `.thread-row` already. */
export function rowGestureHandlers({ mobile, enabled, press, onPress }: {
  mobile: boolean;
  enabled: boolean;
  /** Built with the row's menu opener and its tap action. */
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

/** Makes a drawer row open its actions menu from a gesture: a right-click on
 *  desktop, a long press on mobile. See {@link rowGestureHandlers}. */
export function useRowActionsGesture({ onTap, onPress, enabled }: {
  /** The row's ordinary tap action, normally focusing the thread. */
  onTap?: () => void;
  /** Runs on every press, gesture or not. Carries the row's event prefetch, so
   *  a press on an inline control still warms the cache. */
  onPress?: () => void;
  /** False for a skeleton row: no thread, no menu, and nothing to open. */
  enabled: boolean;
}): RowActionsGesture {
  const openRef = useRef<OverflowMenuOpener | null>(null);
  const mobile = viewportIsMobile.value;
  // `useLongPress` reads both callbacks through refs, so the handlers it
  // returns are stable and an in-flight gesture survives a re-render. A
  // mobile menu is placed against the row, since a finger covers the point.
  const press = useLongPress(
    (row, at) => openRef.current?.(row, viewportIsMobile.value ? undefined : at),
    () => onTap?.(),
  );
  return {
    // Only a row with a live hold may drop its ⋯, or nothing could open it.
    hostOpener: { ref: openRef, trigger: !(mobile && enabled) },
    handlers: rowGestureHandlers({ mobile, enabled, press, onPress }),
  };
}
