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

  const drawerVisible = drawnDrawerShown.value;
  const contentOffset = drawerVisible ? drawnDrawerWidth.value : 0;
  document.documentElement.style.setProperty('--split-ratio', String(ratio));
  document.documentElement.style.setProperty('--content-offset', `${contentOffset}px`);
  document.documentElement.toggleAttribute('data-thread-collapsed', threadCollapsed);
  document.documentElement.toggleAttribute('data-content-collapsed', contentCollapsed);
  document.documentElement.toggleAttribute('data-thread-drawer-open', drawerVisible);
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
