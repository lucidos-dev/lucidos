import { useCallback, useRef } from 'preact/hooks';
import { threadDrawerWidth, threadDrawerOpen, splitRatio, MIN_DRAWER_WIDTH } from '../../store/store';

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

    // Capture panel pane pixel width before drag starts so we can keep it constant
    const splitLayout = contentRow.querySelector('.split-layout') as HTMLElement | null;
    const contentPanePx = splitLayout ? splitLayout.offsetWidth * (1 - splitRatio.value) : 0;
    const onMove = (e: PointerEvent) => {
      if (!dragging.current || !contentRow) return;
      const rect = contentRow.getBoundingClientRect();
      let newWidth = e.clientX - rect.left;
      newWidth = Math.max(MIN_DRAWER_WIDTH, newWidth);
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
      localStorage.setItem('cognos-thread-drawer-width', String(threadDrawerWidth.value));
      localStorage.setItem('cognos-split-ratio', String(splitRatio.value));
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
      onPointerDown={visible ? onPointerDown : undefined}
    />
  );
}
