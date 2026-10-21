import { useRef, useCallback, useEffect, useLayoutEffect } from 'preact/hooks';
import { splitRatio, focusedPane, SPLIT_RATIO_KEY } from '../../store/store';
import { focusPane, toggleSplitFromDivider } from '../../store/actions/pane';
import { homeThreadId } from '../../store/actions/homeThread';
import { clampSplitRatio, splitFitsBothPanes } from './splitHelpers';
import {
  beginPaneResize, drawnDrawerShown, drawnDrawerWidth, drawnSplitRatio, endPaneResize, layoutGeometry, measureLayout,
} from './drawnLayout';
import { splitBounds, THREAD_ROW } from '../../store/paneMinimums';
import { startDividerDrag } from './dividerDrag';
import { createDblClickGate } from '../../utils/dblClickGate';
import type { ComponentChildren } from 'preact';

interface Props {
  threadPane: ComponentChildren;
  contentPane: ComponentChildren;
}

const dividerDblGate = createDblClickGate();

export function SplitLayout({ threadPane, contentPane }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  // The drawn layout needs the window's width and every floor at every moment.
  // The Conversation row's height is rem, so a UI-scale change resizes it too.
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    measureLayout();
    window.addEventListener('resize', measureLayout);
    const observer = new ResizeObserver(measureLayout);
    observer.observe(container);
    const brandRow = document.querySelector(THREAD_ROW);
    if (brandRow) observer.observe(brandRow);
    return () => {
      window.removeEventListener('resize', measureLayout);
      observer.disconnect();
      layoutGeometry.value = null;
    };
  }, []);

  // Home leading the header raises the Conversation floor (paneMinimums). It
  // resizes nothing, so it is measured a frame after the header draws it.
  const homeShown = homeThreadId.value !== null;
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      measureLayout();
    });
    return () => cancelAnimationFrame(frame);
  }, [homeShown]);

  const ratio = drawnSplitRatio.value;

  const onDividerDown = useCallback((e: PointerEvent) => {
    dividerDblGate.record();
    e.preventDefault();
    const container = containerRef.current;
    if (!container) return;

    beginPaneResize();

    // CLAMPED drag: the divider stops at each pane's minimum while the pointer
    // keeps going, so what the user releases is what persists. Nothing corrects
    // it afterwards (ADR 0056 replaced the deferred snap with this).
    //
    // It is also what keeps the collapse states (data-thread-collapsed /
    // data-content-collapsed / data-thread-drawer-open) still through a drag,
    // and more firmly than the snap did: those flip at a ratio of exactly 0 or
    // 1, and the clamp cannot reach either, since both minimums are well inside.
    // So the header icon groups they swap cannot dance between hosts as the
    // pointer wiggles across a pane edge, where the snap merely postponed the
    // flip to release. The 1px floor/ceiling that stood in for this is gone.
    // A split too narrow for both floors has no position but collapsed, so
    // there the drag moves nothing and never draws a squeezed pane.
    //
    // Measured ONCE: the bounds derive from the root font size, which a drag
    // cannot change, and each read is a forced style recalc on the hot path the
    // data-pane-resizing kill-list exists to keep smooth. The container's own
    // width is re-read per move, since the window can resize under the drag.
    const bounds = splitBounds();
    const onMove = (e: PointerEvent) => {
      const rect = container.getBoundingClientRect();
      if (!splitFitsBothPanes(rect.width, bounds)) return;
      splitRatio.value = clampSplitRatio(e.clientX - rect.left, rect.width, bounds);
    };

    startDividerDrag(e, onMove, () => {
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      endPaneResize();
    });
  }, []);

  const onDividerDblClick = useCallback(() => {
    if (!dividerDblGate.allow()) return;
    toggleSplitFromDivider();
  }, []);

  const threadCollapsed = ratio === 0;
  const contentCollapsed = ratio >= 1;

  // Expose split ratio and thread-drawer offset so the header can compute divider position
  // The drawer yields with the Conversation, and for room, without writing
  // `threadDrawerOpen`, so it comes back when the room does.
  const drawerVisible = drawnDrawerShown.value;
  const contentOffset = drawerVisible ? drawnDrawerWidth.value : 0;
  document.documentElement.style.setProperty('--split-ratio', String(ratio));
  document.documentElement.style.setProperty('--content-offset', `${contentOffset}px`);
  document.documentElement.toggleAttribute('data-thread-collapsed', threadCollapsed);
  document.documentElement.toggleAttribute('data-content-collapsed', contentCollapsed);
  document.documentElement.toggleAttribute('data-thread-drawer-open', drawerVisible);
  // Drives the header wash over the focused pane's header segment (shell.css).
  document.documentElement.setAttribute('data-focused-pane', focusedPane.value);

  return (
    <div
      ref={containerRef}
      class="split-layout"
    >
      <div
        class={`pane pane-thread${threadCollapsed ? ' pane-collapsed' : ''}`}
        style={{ flex: threadCollapsed ? '0 0 0%' : contentCollapsed ? 1 : `0 0 ${ratio * 100}%` }}
        onPointerDown={() => focusPane('thread')}
        tabIndex={-1}
      >
        {threadPane}
      </div>
      {/* The divider spans the full pane height — anchoring its tooltip to the
          element border would fling it to the far end of the pane, so it opts into
          pointer-tracking via data-tooltip-follow-cursor. It's the only element
          that does; every other element keeps the border anchor so its tooltip
          always sits fully outside it. */}
      <div
        class={`split-divider ${threadCollapsed || contentCollapsed ? 'collapsed' : ''}`}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize thread and content panes"
        style={contentCollapsed
          ? { marginRight: 'var(--space-xs)' }
          : threadCollapsed
            ? { marginLeft: 'var(--space-xs)' }
            : undefined}
        onPointerDown={onDividerDown}
        onDblClick={onDividerDblClick}
        {...(threadCollapsed || contentCollapsed ? { 'data-tooltip': 'Double-click to expand', 'data-tooltip-follow-cursor': '' } : {})}
      />
      <div
        class={`pane pane-content${contentCollapsed ? ' pane-collapsed' : ''}`}
        style={{ flex: contentCollapsed ? '0 0 0%' : 1 }}
        onPointerDown={() => focusPane('content')}
        tabIndex={-1}
      >
        {contentPane}
      </div>
    </div>
  );
}
