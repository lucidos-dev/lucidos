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

export function DashboardPanel() {
  const open = dashboardPanelOpen.value && viewportIsMobile.value;
  const everOpened = useRef(false);
  if (open) everOpened.current = true;
  const panelRef = useRef<HTMLDivElement>(null);
  const [drawnOpen, setDrawnOpen] = useState(false);

  useLayoutEffect(() => {
    if (open) void panelRef.current?.offsetHeight;
    setDrawnOpen(open);
  }, [open]);

  useLayoutEffect(() => {
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
