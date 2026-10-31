import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import { effect } from '@preact/signals';
import { closeDashboardPanel, dashboardPanelOpen } from '../../store/dashboards';
import {
  collapseDashboardNavEntries,
  reopenDashboardPanelFromHistory,
  toggleDashboardPanel,
} from '../../store/actions/thread-navigation';
import { focusedThreadId, mobileView } from '../../store/store';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { lazyComponent } from '../../utils/lazyComponent';
import { viewportIsMobile } from '../../utils/viewport';
import { EdgeSwipeZones } from '../layout/EdgeSwipeZones';
import { DashboardIcon } from '../shared/icons';
import { ToggleIconButton } from '../shared/ToggleIconButton';

const Dashboard = lazyComponent(() => import('./Dashboard').then((m) => m.Dashboard));
prefetchWhenIdle(Dashboard);

// The panel belongs to the thread pane's thread. A thread change by any route,
// or a viewport leaving the phone layout, ends it. A pane swipe does not.
// Leaving the phone layout also folds the panel's rows out of the history.
// Entering it reopens the panel the history points at, since an iOS cold
// launch can read the layout wrong at first.
//
// A signal effect runs on each write, so a caller that navigates and then
// opens keeps the panel open. The first run only reads the values: a reload
// may have reopened the panel before this module loaded.
let lastThreadId = focusedThreadId.peek();
let lastIsMobile = viewportIsMobile.peek();
effect(() => {
  const threadId = focusedThreadId.value;
  const isMobile = viewportIsMobile.value;
  if (threadId === lastThreadId && isMobile === lastIsMobile) return;
  const layoutChanged = isMobile !== lastIsMobile;
  lastThreadId = threadId;
  lastIsMobile = isMobile;
  closeDashboardPanel();
  if (!layoutChanged) return;
  if (isMobile) reopenDashboardPanelFromHistory();
  else collapseDashboardNavEntries();
});

/** The phone's dashboard button, beside the Lucidos mark in the thread
 *  header. It renders on a phone only, since both headers are mounted and CSS
 *  shows one. */
export function DashboardPanelToggle() {
  if (!viewportIsMobile.value) return null;
  const open = dashboardPanelOpen.value;
  const label = open ? 'Hide dashboard' : 'Show dashboard';
  return (
    <ToggleIconButton class="header-icon dashboard-panel-toggle" onClick={toggleDashboardPanel} aria-label={label} pressed={open}>
      <DashboardIcon />
    </ToggleIconButton>
  );
}

/** The phone dashboard, a layer of the thread pane over the open thread (the
 *  dashboard panel). `MobileSwipeContainer` mounts it inside that pane, so a
 *  pane swipe carries it along and it is still open on return.
 *
 *  It never moves focus, so a thread-aware tile describes the thread underneath.
 *  The first open mounts it, and it then stays mounted, inert and hidden, so a
 *  reopen finds its tiles live. It meets the app header's bottom edge, and the
 *  header stays live above it.
 *
 *  Off the thread pane its dashboard counts as hidden. So it settles only once
 *  that pane has slid in, never on the slide's off-screen answers. */
export function DashboardPanel() {
  const open = dashboardPanelOpen.value && viewportIsMobile.value;
  const everOpened = useRef(false);
  if (open) everOpened.current = true;
  const panelRef = useRef<HTMLDivElement>(null);
  // Lags the first open by one layout pass. The closed frame is styled first,
  // so the fade has a start opacity to move from.
  const [drawnOpen, setDrawnOpen] = useState(false);

  useLayoutEffect(() => {
    if (open) void panelRef.current?.offsetHeight;
    setDrawnOpen(open);
  }, [open]);

  useLayoutEffect(() => {
    // The keyboard belongs to the chat underneath.
    if (open) (document.activeElement as HTMLElement | null)?.blur?.();
  }, [open]);

  if (!everOpened.current) return null;
  return (
    <div ref={panelRef} class={`dashboard-panel${drawnOpen ? ' is-open' : ''}`}>
      <div
        class="dashboard-panel-body"
        role="region"
        aria-label="Dashboard"
        aria-hidden={!open}
        inert={!open}
      >
        <Dashboard dashboard="phone" hidden={!open || mobileView.value !== 'thread'} />
      </div>
      <EdgeSwipeZones />
    </div>
  );
}
