import { useCallback, useRef } from 'preact/hooks';
import { threadDrawerWidth, threadDrawerOpen, splitRatio, MIN_DRAWER_WIDTH, THREAD_DRAWER_WIDTH_KEY, SPLIT_RATIO_KEY } from '../../store/store';
import { beginPaneResize, endPaneResize, computeDrawerSnap, computeSnapRatio, setSplitRatio } from './splitHelpers';

export function DrawerDivider() {
  const visible = threadDrawerOpen.value && splitRatio.value > 0;

  const dragging = useRef(false);

  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    dragging.current = true;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);

    const contentRow = target.parentElement;
    if (!contentRow) return;

    beginPaneResize();

    // Remember the grab-point width so a drag-to-close can restore a usable
    // width for the next open instead of reopening as a sliver.
    const startWidth = threadDrawerWidth.value;

    // Capture panel pane pixel width before drag starts so we can keep it constant
    const splitLayout = contentRow.querySelector('.split-layout') as HTMLElement | null;
    const contentPanePx = splitLayout ? splitLayout.offsetWidth * (1 - splitRatio.value) : 0;
    // Free drag: the drawer lands wherever the pointer drops it. The minimum
    // width is enforced by the deferred snap on release, not mid-drag.
    const onMove = (e: PointerEvent) => {
      if (!dragging.current || !contentRow) return;
      const rect = contentRow.getBoundingClientRect();
      if (rect.width <= 1) return;
      // Upper clamp: with pointer capture, clientX keeps reporting beyond the
      // window edge — without it the drawer lands wider than the container,
      // and at >= MIN_DRAWER_WIDTH no snap fires to pull it back.
      const newWidth = Math.min(Math.max(0, e.clientX - rect.left), rect.width - 1);
      threadDrawerWidth.value = newWidth;

      // Adjust split ratio to keep panel pane at the same pixel width
      const newSplitWidth = rect.width - newWidth;
      if (newSplitWidth > 0 && contentPanePx > 0) {
        const newRatio = Math.max(0, Math.min(1, 1 - contentPanePx / newSplitWidth));
        splitRatio.value = newRatio;
      }
    };

    const cleanup = () => {
      dragging.current = false;
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', cleanup);
      target.removeEventListener('pointercancel', cleanup);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(threadDrawerWidth.value));
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));

      const snap = computeDrawerSnap(threadDrawerWidth.value, MIN_DRAWER_WIDTH);
      // The drag preserves the content pane, so widening the drawer eats the
      // thread pane — which can land below ITS minimum too. Snap it alongside
      // the drawer; widths measured at release (a drawer snap shifts them a
      // little, but only further above the minimum).
      const ratioSnap = splitLayout
        ? computeSnapRatio(splitRatio.value, splitLayout.offsetWidth)
        : null;
      endPaneResize(snap === null && ratioSnap === null ? undefined : () => {
        if (snap === 'close') {
          threadDrawerOpen.value = false; // persisted by the store effect
          threadDrawerWidth.value = Math.max(startWidth, MIN_DRAWER_WIDTH);
        } else if (snap === 'min') {
          threadDrawerWidth.value = MIN_DRAWER_WIDTH;
        }
        if (snap !== null) {
          localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(threadDrawerWidth.value));
        }
        if (ratioSnap !== null) setSplitRatio(ratioSnap);
      });
    };

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    target.addEventListener('pointermove', onMove);
    target.addEventListener('pointerup', cleanup);
    target.addEventListener('pointercancel', cleanup);
  }, []);

  return (
    <div
      class={`drawer-divider${visible ? '' : ' drawer-divider-collapsed'}`}
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize thread drawer"
      onPointerDown={visible ? onPointerDown : undefined}
    />
  );
}
