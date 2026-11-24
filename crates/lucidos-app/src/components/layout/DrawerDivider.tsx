import { useCallback } from 'preact/hooks';
import { threadDrawerWidth, splitRatio, THREAD_DRAWER_WIDTH_KEY, SPLIT_RATIO_KEY } from '../../store/store';
import { minDrawerWidth, minThreadPanePx, splitBounds } from '../../store/paneMinimums';
import { clampToRange, clampSplitRatio } from './splitHelpers';
import { beginPaneResize, drawnDrawerShown, drawnSplitRatio, endPaneResize } from './drawnLayout';
import { startDividerDrag } from './dividerDrag';

export function DrawerDivider() {
  const visible = drawnDrawerShown.value;

  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    const contentRow = (e.currentTarget as HTMLElement).parentElement;
    if (!contentRow) return;

    beginPaneResize();

    const splitLayout = contentRow.querySelector('.split-layout') as HTMLElement | null;
    const contentPanePx = splitLayout ? splitLayout.offsetWidth * (1 - drawnSplitRatio.peek()) : 0;

    const floor = minDrawerWidth();
    const threadFloor = minThreadPanePx();
    const bounds = splitBounds();
    const onMove = (e: PointerEvent) => {
      const rect = contentRow.getBoundingClientRect();
      if (rect.width <= 1) return;
      const newWidth = clampToRange(
        e.clientX - rect.left,
        floor,
        rect.width - threadFloor - contentPanePx,
      );
      threadDrawerWidth.value = newWidth;

      const newSplitWidth = rect.width - newWidth;
      if (newSplitWidth > 0 && contentPanePx > 0) {
        splitRatio.value = clampSplitRatio(newSplitWidth - contentPanePx, newSplitWidth, bounds);
      }
    };

    startDividerDrag(e, onMove, () => {
      localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(threadDrawerWidth.value));
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      endPaneResize();
    });
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
