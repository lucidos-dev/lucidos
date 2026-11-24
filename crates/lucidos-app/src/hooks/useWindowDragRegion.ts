import { useEffect } from 'preact/hooks';
import { isTauri } from '../utils/platform';
import { startWindowDrag, toggleWindowMaximize } from '../utils/tauri';

export const WINDOW_DRAG_THRESHOLD_PX = 4;

interface WindowDragOptions {
  canStart?: (target: HTMLElement) => boolean;
  maximize?: boolean;
}

export function useWindowDragRegion(
  ref: { current: HTMLElement | null },
  { canStart, maximize }: WindowDragOptions = {},
): void {
  useEffect(() => {
    const el = ref.current;
    if (!el || !isTauri()) return;

    let armed = false;
    let startX = 0;
    let startY = 0;

    const disarm = () => {
      armed = false;
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', disarm);
      window.removeEventListener('pointercancel', disarm);
    };

    function onPointerMove(e: PointerEvent) {
      if (!armed) return;
      if ((e.buttons & 1) === 0) { disarm(); return; }
      if (Math.hypot(e.clientX - startX, e.clientY - startY) < WINDOW_DRAG_THRESHOLD_PX) return;
      disarm();
      startWindowDrag().catch(() => {});
    }

    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0) return;
      if (canStart && !canStart(e.target as HTMLElement)) return;
      armed = true;
      startX = e.clientX;
      startY = e.clientY;
      window.addEventListener('pointermove', onPointerMove);
      window.addEventListener('pointerup', disarm);
      window.addEventListener('pointercancel', disarm);
    };

    const onDblClick = maximize
      ? (e: MouseEvent) => {
          if (canStart && !canStart(e.target as HTMLElement)) return;
          toggleWindowMaximize().catch(() => {});
        }
      : null;

    el.addEventListener('pointerdown', onPointerDown);
    if (onDblClick) el.addEventListener('dblclick', onDblClick);

    return () => {
      el.removeEventListener('pointerdown', onPointerDown);
      if (onDblClick) el.removeEventListener('dblclick', onDblClick);
      disarm();
    };
  }, [ref, canStart, maximize]);
}
