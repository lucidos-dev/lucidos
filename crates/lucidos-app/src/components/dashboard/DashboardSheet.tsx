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

/** The phone's dashboard button, beside the Lucidos mark in the thread
 *  header. It renders on a phone only, since both headers are mounted and CSS
 *  shows one. */
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

function visibleToggle(): HTMLElement | null {
  return [...document.querySelectorAll<HTMLElement>('.mobile-thread-header .dashboard-sheet-toggle')]
    .find((el) => el.getBoundingClientRect().height > 0) ?? null;
}

/** The phone dashboard drawn over the open thread (the dashboard sheet).
 *
 *  It never moves focus, so a thread-aware tile describes the thread underneath.
 *  The first open mounts it, and it then stays mounted, inert and hidden, so a
 *  reopen finds its tiles live. It hangs below the app header, which stays
 *  clear of the dim. */
export function DashboardSheet() {
  const open = dashboardSheetOpen.value && viewportIsMobile.value;
  const everOpened = useRef(false);
  if (open) everOpened.current = true;
  const panelRef = useRef<HTMLDivElement>(null);
  const [toggle, setToggle] = useState<HTMLElement | null>(null);
  // Lags the first open by one layout pass. The closed frame is styled first,
  // so the rise has a start to move from.
  const [drawnOpen, setDrawnOpen] = useState(false);

  useLayoutEffect(() => {
    if (open) void panelRef.current?.offsetHeight;
    setDrawnOpen(open);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    // The keyboard belongs to the chat underneath.
    (document.activeElement as HTMLElement | null)?.blur?.();
    setToggle(visibleToggle());
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    pushOverlay({ id: SHEET_OVERLAY_ID, dismiss: closeDashboardSheet, hasPanel: true });
    return () => removeOverlay(SHEET_OVERLAY_ID);
  }, [open]);

  useDismissOnOutside(
    open,
    panelRef,
    toggle,
    () => { closeDashboardSheet(); },
    () => topPanelOverlay()?.id === SHEET_OVERLAY_ID,
  );

  if (!everOpened.current || typeof document === 'undefined') return null;
  return createPortal(
    <div class={`dashboard-sheet${drawnOpen ? ' is-open' : ''}`}>
      <div class="dashboard-sheet-dim" />
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
