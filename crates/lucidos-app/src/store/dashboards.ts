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

/** Which face of Home a phone shows in its middle pane: the dashboard, or the
 *  Home chat slid up over it. A desktop shows the chat, and the dashboard in
 *  its rail. */
export type HomeFace = 'dashboard' | 'chat';

export const homeFace = signal<HomeFace>('chat');

let dashboardLanding = false;

/** The next arrival in Home lands on its dashboard face. The Home icon, the
 *  Lucidos menu's Home row and a cold start with no stored focus ask for it.
 *  Every other way in, such as a link or a notification about a reply, lands
 *  on the chat face, where that reply is. */
export function landNextHomeOnDashboard(): void {
  dashboardLanding = true;
}

/** The face an arrival in Home lands on, spending the request above. */
export function takeHomeLanding(): HomeFace {
  const face: HomeFace = dashboardLanding ? 'dashboard' : 'chat';
  dashboardLanding = false;
  return face;
}
