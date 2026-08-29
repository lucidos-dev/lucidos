import { useRef } from 'preact/hooks';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import type { OverflowMenuOpener } from '../shared/OverflowMenu';

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
  /** Handed to the row's overflow menu. Set only on mobile, where its presence
   *  is what drops the ⋯ trigger and makes this gesture the way in. */
  openRef?: { current: OverflowMenuOpener | null };
  handlers: RowGestureHandlers;
}

/** A press that began on an inline control belongs to that control. The pin and
 *  the family disclosure both live inside the row, and both would otherwise
 *  arm the row's hold underneath them: holding the pin would pin the thread AND
 *  open the menu. */
function startsOnControl(e: { target: EventTarget | null }): boolean {
  return !!(e.target as Element | null)?.closest?.('button');
}

/** Makes a long press on a drawer row open that row's actions menu, on mobile.
 *
 *  The row's ⋯ trigger is a 31x27px box against the pane's right edge, which is
 *  the hardest place on a phone to reach. So the mobile row drops it and the
 *  whole row becomes the target instead. Desktop is untouched: this returns the
 *  row's ordinary tap handlers and no `openRef`, so the ⋯ still renders.
 *
 *  A scroll never opens the menu: `useLongPress` cancels the hold once the
 *  pointer travels 10px, and again on `pointercancel`. A fired hold swallows
 *  its own paired click, so the row does not also open the thread.
 *
 *  `onContextMenu` is wired on mobile only. Android fires it for a long press,
 *  and would draw the browser's own menu over ours. iOS does not, the callout
 *  being suppressed on `.thread-row` already. */
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
  // `useLongPress` reads both callbacks through refs, so the handlers it
  // returns are stable and an in-flight gesture survives a re-render.
  const press = useLongPress(
    (row) => openRef.current?.(row),
    () => onTap?.(),
  );

  if (!viewportIsMobile.value || !enabled) {
    return {
      handlers: {
        onPointerDown: enabled ? () => onPress?.() : undefined,
        onClick: enabled ? () => onTap?.() : undefined,
      },
    };
  }

  return {
    openRef,
    handlers: {
      onPointerDown: (e) => {
        onPress?.();
        if (startsOnControl(e)) return;
        press.onPointerDown(e);
      },
      onPointerMove: press.onPointerMove,
      onPointerUp: press.onPointerUp,
      onPointerLeave: press.onPointerLeave,
      onPointerCancel: press.onPointerCancel,
      onContextMenu: (e) => { if (!startsOnControl(e)) press.onContextMenu(e); },
      onClick: press.onClick,
    },
  };
}
