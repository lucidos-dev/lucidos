import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { effect } from '@preact/signals';
import { createPortal } from 'preact/compat';
import { useDismissOnOutside } from '../../hooks/useAnchoredPopover';
import { closeDashboardSheet, dashboardSheetOpen, toggleDashboardSheet } from '../../store/dashboards';
import { pushOverlay, removeOverlay, topPanelOverlay } from '../../store/overlayStack';
import { focusedThreadId, mobileView } from '../../store/store';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { lazyComponent } from '../../utils/lazyComponent';
import { viewportIsMobile } from '../../utils/viewport';
import { DashboardIcon } from '../shared/icons';

const Dashboard = lazyComponent(() => import('./Dashboard').then((m) => m.Dashboard));
prefetchWhenIdle(Dashboard);

const SHEET_OVERLAY_ID = 'dashboard-sheet';

// The sheet belongs to the thread pane's thread. A pane swipe, a thread change
// by any route, or a viewport leaving the phone layout all end it. A signal
// effect runs on each write, so a caller that navigates and then opens keeps
// the sheet open.
effect(() => {
  void [mobileView.value, focusedThreadId.value, viewportIsMobile.value];
  closeDashboardSheet();
});

/** The phone's dashboard button, at the trailing end of the thread title row.
 *  It renders on a phone only, since both title rows are mounted and CSS shows
 *  one. */
export function DashboardSheetToggle() {
  if (!viewportIsMobile.value) return null;
  const open = dashboardSheetOpen.value;
  const label = open ? 'Hide dashboard' : 'Show dashboard';
  return (
    <button
      type="button"
      class="icon-btn header-icon dashboard-sheet-toggle"
      onClick={toggleDashboardSheet}
      aria-label={label}
      aria-pressed={open}
    >
      <DashboardIcon />
    </button>
  );
}

/** Where the visible title row sits, and its toggle. The sheet hangs from the
 *  row's real bottom, so a two-line title or a larger UI scale moves it. */
interface TitleRow {
  top: number;
  bottom: number;
  toggle: HTMLElement | null;
  row: HTMLElement;
}

function visibleTitleRow(): TitleRow | null {
  for (const row of document.querySelectorAll<HTMLElement>('.mobile-thread-title-row')) {
    const rect = row.getBoundingClientRect();
    if (rect.height > 0) {
      return { top: rect.top, bottom: rect.bottom, toggle: row.querySelector<HTMLElement>('.dashboard-sheet-toggle'), row };
    }
  }
  return null;
}

/** Keeps the previous edges when nothing moved, so a scroll storm re-renders nothing. */
function sameEdges(prev: TitleRow | null, next: TitleRow | null): TitleRow | null {
  if (prev && next && prev.top === next.top && prev.bottom === next.bottom && prev.toggle === next.toggle && prev.row === next.row) return prev;
  return next;
}

/** The phone dashboard drawn over the open thread (the dashboard sheet).
 *
 *  It never moves focus, so a thread-aware tile describes the thread underneath.
 *  The first open mounts it, and it then stays mounted, inert and hidden, so a
 *  reopen finds its tiles live. The title row stays clear of both dims. */
export function DashboardSheet() {
  const open = dashboardSheetOpen.value && viewportIsMobile.value;
  const everOpened = useRef(false);
  if (open) everOpened.current = true;
  const panelRef = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState<TitleRow | null>(null);
  // Lags the first open by one layout pass. The closed frame is styled first,
  // so the rise has a start to move from.
  const [drawnOpen, setDrawnOpen] = useState(false);

  useLayoutEffect(() => {
    if (open) void panelRef.current?.offsetHeight;
    setDrawnOpen(open);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    // The keyboard belongs to the chat underneath.
    (document.activeElement as HTMLElement | null)?.blur?.();
    const measure = () => setEdges((prev) => sameEdges(prev, visibleTitleRow()));
    measure();
    window.addEventListener('resize', measure);
    // The row is sticky: the thread under the dim can scroll it, and a header
    // that hides or shows moves it with no size change.
    window.addEventListener('scroll', measure, true);
    window.visualViewport?.addEventListener('resize', measure);
    const row = visibleTitleRow()?.row;
    const observer = row && typeof ResizeObserver === 'function' ? new ResizeObserver(measure) : null;
    if (row) observer?.observe(row);
    return () => {
      window.removeEventListener('resize', measure);
      window.removeEventListener('scroll', measure, true);
      window.visualViewport?.removeEventListener('resize', measure);
      observer?.disconnect();
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    pushOverlay({ id: SHEET_OVERLAY_ID, dismiss: closeDashboardSheet, hasPanel: true });
    return () => removeOverlay(SHEET_OVERLAY_ID);
  }, [open]);

  useDismissOnOutside(
    open,
    panelRef,
    edges?.toggle ?? null,
    () => { closeDashboardSheet(); },
    () => topPanelOverlay()?.id === SHEET_OVERLAY_ID,
  );

  if (!everOpened.current || typeof document === 'undefined') return null;
  const style = edges ? { '--sheet-row-top': `${edges.top}px`, '--sheet-row-bottom': `${edges.bottom}px` } : undefined;
  return createPortal(
    <div class={`dashboard-sheet${drawnOpen ? ' is-open' : ''}`} style={style}>
      <div class="dashboard-sheet-dim dashboard-sheet-dim-top" />
      <div class="dashboard-sheet-dim dashboard-sheet-dim-gap" />
      <div
        ref={panelRef}
        class="dashboard-sheet-panel"
        role="region"
        aria-label="Dashboard"
        data-overlay-panel={SHEET_OVERLAY_ID}
        aria-hidden={!open}
        inert={!open}
      >
        <Dashboard dashboard="phone" hidden={!open} />
      </div>
    </div>,
    document.body,
  );
}
