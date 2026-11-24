
import { signal } from '@preact/signals';
import type { DashboardView } from '../api/client/widgets';
import type { Dashboard } from '../generated/thread-event-wire';
import { viewportIsMobile } from '../utils/viewport';
import type { Loadable } from './types';

export const dashboards = signal<ReadonlyMap<Dashboard, Loadable<DashboardView>>>(new Map());

export function dashboardFor(dashboard: Dashboard): Loadable<DashboardView> {
  return dashboards.value.get(dashboard) ?? { status: 'not-loaded' };
}

export function currentDashboard(): Dashboard {
  return viewportIsMobile.value ? 'phone' : 'desktop';
}

export const dashboardPanelOpen = signal(false);

export function openDashboardPanel(): void {
  dashboardPanelOpen.value = true;
}

export function closeDashboardPanel(): void {
  dashboardPanelOpen.value = false;
}
