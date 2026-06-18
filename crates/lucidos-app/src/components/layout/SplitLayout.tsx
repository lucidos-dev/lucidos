import { useRef, useCallback } from 'preact/hooks';
import { splitRatio, threadDrawerOpen, threadDrawerWidth, focusedPane, SPLIT_RATIO_KEY } from '../../store/store';
import { focusPane } from '../../store/actions/pane';
import { setSplitRatio, computeSnapRatio, beginPaneResize, endPaneResize, DEFAULT_SPLIT_RATIO } from './splitHelpers';
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

    // Free drag: the divider lands wherever the pointer drops it. Minimum
    // widths are enforced by the deferred snap on release, not mid-drag.
    // The 1px floor/ceiling keeps the ratio off exactly 0/1 while dragging:
    // the collapse states (data-thread-collapsed / data-content-collapsed /
    // data-thread-drawer-open) flip only at the post-release snap, so the
    // header icon groups they swap can't dance between hosts as the pointer
    // wiggles across a pane edge.
    const onMove = (e: PointerEvent) => {
      if (!dragging.current || !container) return;
      const rect = container.getBoundingClientRect();
      if (rect.width <= 1) return;
      const chatPx = Math.min(Math.max(e.clientX - rect.left, 1), rect.width - 1);
      splitRatio.value = chatPx / rect.width;
    };

    const cleanup = () => {
      dragging.current = false;
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', cleanup);
      target.removeEventListener('pointercancel', cleanup);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem(SPLIT_RATIO_KEY, String(splitRatio.value));
      const snapTo = computeSnapRatio(splitRatio.value, container.offsetWidth);
      endPaneResize(snapTo === null ? undefined : () => setSplitRatio(snapTo));
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
  // Drives the accent line under the focused pane's header region (shell.css).
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
          ? { marginRight: '0.25rem' }
          : threadCollapsed
            ? { marginLeft: '0.25rem' }
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
