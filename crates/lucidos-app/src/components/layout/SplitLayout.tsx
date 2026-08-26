import { useRef, useCallback, useLayoutEffect } from 'preact/hooks';
import { splitRatio, threadDrawerOpen, threadDrawerWidth, focusedPane, SPLIT_RATIO_KEY } from '../../store/store';
import { focusPane } from '../../store/actions/pane';
import { setSplitRatio, clampSplitRatio, migratedSplitRatio, beginPaneResize, endPaneResize, DEFAULT_SPLIT_RATIO } from './splitHelpers';
import { splitBounds } from '../../store/paneMinimums';
import { createDblClickGate } from '../../utils/dblClickGate';
import type { ComponentChildren } from 'preact';

interface Props {
  threadPane: ComponentChildren;
  contentPane: ComponentChildren;
}

const dividerDblGate = createDblClickGate();

export function SplitLayout({ threadPane, contentPane }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  // Bring a persisted ratio that is no longer legal up to the floor, ONCE, as
  // soon as the split has a width. `migratedSplitRatio` explains why nothing
  // else would. A ResizeObserver rather than a bare layout-effect read: the
  // first frame can measure 0 while the shell is still sizing, and a migration
  // that silently skips itself there is no migration at all. It disconnects on
  // the first usable width, so a later window resize is untouched.
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const migrate = (width: number) => {
      if (width <= 2) return false;
      const next = migratedSplitRatio(splitRatio.value, width, splitBounds());
      if (next !== null) setSplitRatio(next);
      return true;
    };
    if (migrate(container.getBoundingClientRect().width)) return;
    const observer = new ResizeObserver(() => {
      if (migrate(container.getBoundingClientRect().width)) observer.disconnect();
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const onDividerDown = useCallback((e: PointerEvent) => {
    dividerDblGate.record();
    e.preventDefault();
    dragging.current = true;
    const container = containerRef.current;
    const target = e.currentTarget as HTMLElement;
    if (!container) return;

    // Capture pointer so we get all move/up events even if pointer leaves the element
    target.setPointerCapture(e.pointerId);
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
    //
    // Measured ONCE: the bounds derive from the root font size, which a drag
    // cannot change, and each read is a forced style recalc on the hot path the
    // data-pane-resizing kill-list exists to keep smooth. The container's own
    // width is re-read per move, since the window can resize under the drag.
    const bounds = splitBounds();
    const onMove = (e: PointerEvent) => {
      if (!dragging.current || !container) return;
      const rect = container.getBoundingClientRect();
      if (rect.width <= 1) return;
      splitRatio.value = clampSplitRatio(e.clientX - rect.left, rect.width, bounds);
    };

    const cleanup = () => {
      dragging.current = false;
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', cleanup);
      target.removeEventListener('pointercancel', cleanup);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      endPaneResize();
    };

    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    target.addEventListener('pointermove', onMove);
    target.addEventListener('pointerup', cleanup);
    target.addEventListener('pointercancel', cleanup);
  }, []);

  const onDividerDblClick = useCallback(() => {
    if (!dividerDblGate.allow()) return;
    const ratio = splitRatio.value;
    const expanding = ratio === 0 || ratio >= 1;
    setSplitRatio(expanding ? DEFAULT_SPLIT_RATIO : 0);
  }, []);

  const ratio = splitRatio.value;
  const threadCollapsed = ratio === 0;
  const contentCollapsed = ratio >= 1;

  // Expose split ratio and thread-drawer offset so the header can compute divider position
  // Hide drawer when thread pane is collapsed but don't mutate threadDrawerOpen — so it restores on expand
  const drawerVisible = threadDrawerOpen.value && !threadCollapsed;
  const contentOffset = drawerVisible ? threadDrawerWidth.value : 0;
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
