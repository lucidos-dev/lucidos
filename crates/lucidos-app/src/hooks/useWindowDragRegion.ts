import { useEffect } from 'preact/hooks';
import { isTauri } from '../utils/platform';
import { startWindowDrag, toggleWindowMaximize } from '../utils/tauri';

/** Pixels the pointer must travel while pressed before a window drag starts.
 *  Small enough to feel immediate, large enough that a plain click — or either
 *  click of a double-click — never crosses it, so click / dblclick handlers
 *  (the header's double-click → pane-maximize) keep firing untouched. */
export const WINDOW_DRAG_THRESHOLD_PX = 4;

interface WindowDragOptions {
  /** Gate: return false to NOT begin a drag for a press on this target (e.g. the
   *  header excludes its buttons/inputs). Omitted → any press arms a drag. */
  canStart?: (target: HTMLElement) => boolean;
  /** When true, a double-click on the element toggles the window's maximized
   *  (macOS zoom) state. Used by the reclaimed title-bar strip only. */
  maximize?: boolean;
}

/**
 * Make an element a window drag region in the Tauri desktop build, replacing
 * `data-tauri-drag-region` (whose internal `plugin:window|start_dragging` IPC is
 * denied by our capability ACL — see `lib.rs::start_window_drag`). A press that
 * then moves past `WINDOW_DRAG_THRESHOLD_PX` starts a native window drag via the
 * always-allowed `start_window_drag` app command; pure clicks and double-clicks
 * never cross the threshold, so they still reach the page's own handlers.
 *
 * No-op off Tauri (web / PWA / mobile): the listeners are never attached.
 *
 * The element must be mounted by the render that calls this, because the effect
 * reads `ref.current` once and its deps never change afterwards. A CONDITIONALLY
 * rendered region therefore cannot pass a `useRef`, which would still hold null
 * from the render that rendered nothing: pass a state-backed element instead
 * (`useState` + a callback `ref`, wrapped so the object identity changes with
 * it), so the effect re-runs when the element appears. `WorkspacePicker` is the
 * worked example.
 */
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
      // Button released without us seeing pointerup (e.g. focus loss) — stop.
      if ((e.buttons & 1) === 0) { disarm(); return; }
      if (Math.hypot(e.clientX - startX, e.clientY - startY) < WINDOW_DRAG_THRESHOLD_PX) return;
      disarm();
      // Best-effort: a failed drag just leaves the window where it was; the next
      // press retries. No user-facing action to surface an error against.
      startWindowDrag().catch(() => {});
    }

    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0) return; // primary button only
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
          // Best-effort like startWindowDrag above: a failed toggle leaves the
          // window as-is (visibly unchanged); the next double-click retries.
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
