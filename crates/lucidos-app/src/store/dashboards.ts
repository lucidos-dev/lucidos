/** Dashboard state (ADR 0496): the tiles of each dashboard, `phone` and
 *  `desktop`, as the engine folds them.
 *
 *  The client keeps the last answer per dashboard. A dashboard event arriving
 *  over SSE re-reads that dashboard. */

import { signal } from '@preact/signals';
import type { DashboardView } from '../api/client/widgets';
import type { Dashboard } from '../generated/thread-event-wire';
import { viewportIsMobile } from '../utils/viewport';
import type { Loadable } from './types';

export const dashboards = signal<ReadonlyMap<Dashboard, Loadable<DashboardView>>>(new Map());

export function dashboardFor(dashboard: Dashboard): Loadable<DashboardView> {
  return dashboards.value.get(dashboard) ?? { status: 'not-loaded' };
}

/** The dashboard this layout shows: the phone's swipe layout shows `phone`,
 *  and the split layout shows `desktop`, a tablet included. */
export function currentDashboard(): Dashboard {
  return viewportIsMobile.value ? 'phone' : 'desktop';
}

/** The dashboard sheet (ADR 0496 amendment): the phone dashboard drawn over
 *  the open thread. It never moves focus, so a thread-aware tile describes
 *  the thread underneath. Not persisted: a reload opens with it closed. */
export const dashboardSheetOpen = signal(false);

export function openDashboardSheet(): void {
  dashboardSheetOpen.value = true;
}

export function closeDashboardSheet(): void {
  dashboardSheetOpen.value = false;
}

export function toggleDashboardSheet(): void {
  dashboardSheetOpen.value = !dashboardSheetOpen.value;
}
