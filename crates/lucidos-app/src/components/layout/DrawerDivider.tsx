import { useCallback } from 'preact/hooks';
import { threadDrawerWidth, threadDrawerOpen, splitRatio, THREAD_DRAWER_WIDTH_KEY, SPLIT_RATIO_KEY } from '../../store/store';
import { minDrawerWidth, minThreadPanePx, splitBounds } from '../../store/paneMinimums';
import { beginPaneResize, endPaneResize, clampToRange, clampSplitRatio } from './splitHelpers';
import { startDividerDrag } from './dividerDrag';

export function DrawerDivider() {
  const visible = threadDrawerOpen.value && splitRatio.value > 0;

  const onPointerDown = useCallback((e: PointerEvent) => {
    e.preventDefault();
    const contentRow = (e.currentTarget as HTMLElement).parentElement;
    if (!contentRow) return;

    beginPaneResize();

    // Capture panel pane pixel width before drag starts so we can keep it constant
    const splitLayout = contentRow.querySelector('.split-layout') as HTMLElement | null;
    const contentPanePx = splitLayout ? splitLayout.offsetWidth * (1 - splitRatio.value) : 0;

    // CLAMPED drag, both ends (ADR 0056). Narrowing stops at the drawer's own
    // floor, which is what its header row needs. WIDENING stops too, and that
    // end is the less obvious one: the drag deliberately holds the content pane
    // at a constant pixel width, so every pixel the drawer gains comes out of
    // the THREAD pane, which has a minimum of its own. Without the ceiling a
    // wide drag squeezed it to nothing.
    //
    // Nothing corrects any of this on release, which is also what holds the
    // collapse-state attributes still through the gesture: the drawer's own
    // stays put because only its toggle writes it, and the split panes' cannot
    // flip because `clampSplitRatio` guarantees a ratio strictly inside (0, 1).
    //
    // Measured ONCE, like the split divider's bounds: they derive from the root
    // font size, which a drag cannot change, and each read forces a style
    // recalc on a per-pointermove path.
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

      // Keep the content pane at the pixel width it had at grab time: the
      // remaining space is the split's, and the thread pane takes what the
      // content pane does not. Routed through the same clamp the split divider
      // uses, so the two dividers cannot disagree about the panes' floors and
      // so this path inherits the strictly-inside-(0, 1) guarantee.
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
