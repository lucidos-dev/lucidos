import { useRef, useCallback, useEffect } from 'preact/hooks';
import { splitRatio, threadDrawerOpen, threadDrawerWidth } from '../../store/store';
import { animateBrandReturn, triggerSnapAnimate, setSplitRatio, DEFAULT_SPLIT_RATIO } from './splitHelpers';
import { createDblClickGate } from '../../utils/dblClickGate';
import type { ComponentChildren } from 'preact';

interface Props {
  threadPane: ComponentChildren;
  contentPane: ComponentChildren;
}

const MIN_PANE_PX = 300;
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

    const onMove = (e: PointerEvent) => {
      if (!dragging.current || !container) return;
      const rect = container.getBoundingClientRect();
      const totalWidth = rect.width;
      let chatPx = e.clientX - rect.left;

      const prevRatio = splitRatio.value;

      // Collapse if dragged past minimum
      if (chatPx < MIN_PANE_PX / 2) { chatPx = 0; }
      else if (chatPx < MIN_PANE_PX) chatPx = MIN_PANE_PX;
      if (totalWidth - chatPx < MIN_PANE_PX / 2) chatPx = totalWidth;
      else if (totalWidth - chatPx < MIN_PANE_PX) chatPx = totalWidth - MIN_PANE_PX;

      const newRatio = chatPx / totalWidth;

      // Animate snap when crossing collapse/expand boundaries
      const chatSnapped = (prevRatio === 0) !== (newRatio === 0);
      const panelSnapped = (prevRatio >= 1) !== (newRatio >= 1);
      if (chatSnapped || panelSnapped) {
        triggerSnapAnimate();
        // Force reflow so the browser computes the "before" state with the
        // transition property applied.  Without this, the class addition and
        // the value change land in the same frame and the transition is skipped.
        void container.offsetWidth;
      }

      // Animate brand returning when crossing from collapsed to expanded
      if (prevRatio === 0 && newRatio > 0) {
        animateBrandReturn(newRatio);
      }

      splitRatio.value = newRatio;
    };

    const cleanup = () => {
      dragging.current = false;
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', cleanup);
      target.removeEventListener('pointercancel', cleanup);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem('cognos-split-ratio', String(splitRatio.value));
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

  // Track thread pane pixel width to progressively hide header selectors
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const update = () => {
      const threadPx = container.offsetWidth * splitRatio.value;
      document.documentElement.toggleAttribute('data-thread-narrow', threadPx > 0 && threadPx < 400);
      document.documentElement.toggleAttribute('data-thread-very-narrow', threadPx > 0 && threadPx < 300);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(container);
    return () => ro.disconnect();
  }, [ratio]);

  return (
    <div
      ref={containerRef}
      class="split-layout"
    >
      <div
        class={`pane pane-thread${threadCollapsed ? ' pane-collapsed' : ''}`}
        style={{ flex: threadCollapsed ? '0 0 0%' : contentCollapsed ? 1 : `0 0 ${ratio * 100}%` }}
      >
        {threadPane}
      </div>
      <div
        class={`split-divider ${threadCollapsed || contentCollapsed ? 'collapsed' : ''}`}
        style={contentCollapsed
          ? { marginRight: '0.25rem' }
          : threadCollapsed
            ? { marginLeft: '0.25rem' }
            : undefined}
        onPointerDown={onDividerDown}
        onDblClick={onDividerDblClick}
        {...(threadCollapsed || contentCollapsed ? { 'data-tooltip': 'Double-click to expand' } : {})}
      />
      <div
        class={`pane pane-content${contentCollapsed ? ' pane-collapsed' : ''}`}
        style={{ flex: contentCollapsed ? '0 0 0%' : 1 }}
      >
        {contentPane}
      </div>
    </div>
  );
}
