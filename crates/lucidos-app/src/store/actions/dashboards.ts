/** What the dashboards read and change, and how SSE keeps them fresh (ADR 0496). */

import {
  addToDashboardApi,
  arrangeDashboardApi,
  fetchDashboard,
  removeFromDashboardApi,
  type ArrangedTile,
  type DashboardTile,
  type DashboardTileRef,
} from '../../api/client/widgets';
import type { Dashboard } from '../../generated/thread-event-wire';
import { errorDetail } from '../../utils/errorDetail';
import { showToast } from '../store';
import { dashboardFor, dashboards } from '../dashboards';
import { loadingIfFresh, toFailed } from '../types';

/** What a tile's toasts call it: its `kind` on the wire. */
type TileNoun = DashboardTile['kind'];

function setDashboard(dashboard: Dashboard, value: ReturnType<typeof dashboardFor>): void {
  const next = new Map(dashboards.value);
  next.set(dashboard, value);
  dashboards.value = next;
}

/** Per dashboard, the newest read started. Only its answer is applied, so an
 *  older read that lands late cannot put back a tile a newer one removed. */
const newestRead = new Map<Dashboard, number>();
let lastTicket = 0;

/** Read one dashboard. A re-read keeps the tiles on screen. */
export function rereadDashboard(dashboard: Dashboard): Promise<void> {
  const ticket = ++lastTicket;
  newestRead.set(dashboard, ticket);
  setDashboard(dashboard, loadingIfFresh(dashboardFor(dashboard)));
  return fetchDashboard(dashboard).then(
    (data) => {
      if (newestRead.get(dashboard) === ticket) setDashboard(dashboard, { status: 'loaded', data });
    },
    (error) => {
      if (newestRead.get(dashboard) !== ticket) return;
      // A re-read that fails keeps what it had; only a first read fails.
      if (dashboardFor(dashboard).status !== 'loaded') setDashboard(dashboard, toFailed(error));
    },
  );
}

/** What the arrange call carries: identity and size, nothing else. */
export function toArranged(tiles: readonly DashboardTile[]): ArrangedTile[] {
  return tiles.map((t) => ({
    app_id: t.app_id,
    ...(t.kind === 'widget' && t.params ? { params: t.params } : {}),
    size: t.size,
  }));
}

/** Arrange one dashboard: the tiles in their new order, with their sizes. The
 *  dashboard shows it at once. One call writes one `DashboardArranged`, and a
 *  refusal puts the stored dashboard back. */
export async function arrangeDashboard(dashboard: Dashboard, tiles: DashboardTile[]): Promise<void> {
  const current = dashboardFor(dashboard);
  if (current.status !== 'loaded') return;
  const ticket = ++lastTicket;
  newestRead.set(dashboard, ticket);
  setDashboard(dashboard, { status: 'loaded', data: { ...current.data, tiles } });
  try {
    const data = await arrangeDashboardApi(dashboard, toArranged(tiles));
    if (newestRead.get(dashboard) === ticket) setDashboard(dashboard, { status: 'loaded', data });
  } catch (error) {
    showToast(`Couldn't arrange the ${dashboard} dashboard: ${errorDetail(error)}`, 'error');
    void rereadDashboard(dashboard);
  }
}

/** "Add to dashboard": a tile on that dashboard. The engine makes a thread's
 *  own widget reusable in the same action, and the toast says so. Deleting
 *  that thread then never empties the tile. `noun` names what the toast calls
 *  the tile: a widget, or an app shortcut. */
export async function addToDashboard(dashboard: Dashboard, tile: DashboardTileRef, name: string, noun: TileNoun): Promise<void> {
  try {
    const answer = await addToDashboardApi(dashboard, tile);
    showToast(answer.made_reusable
      ? `Added "${name}" to the ${dashboard} dashboard and made it reusable, so its tile stays if its thread is deleted.`
      : `Added "${name}" to the ${dashboard} dashboard.`, 'success');
  } catch (error) {
    showToast(`Couldn't add ${noun} "${name}" to the ${dashboard} dashboard: ${errorDetail(error)}`, 'error');
  }
}

/** "Remove from dashboard": the tile goes, the widget or app and its files stay. */
export async function removeFromDashboard(dashboard: Dashboard, tile: DashboardTileRef, name: string, noun: TileNoun): Promise<void> {
  try {
    await removeFromDashboardApi(dashboard, tile);
  } catch (error) {
    showToast(`Couldn't remove ${noun} "${name}" from the ${dashboard} dashboard: ${errorDetail(error)}`, 'error');
  }
}

/** A dashboard event arrived: re-read that dashboard, if this device read it. */
export function onDashboardEvent(dashboard: Dashboard): void {
  if (dashboardFor(dashboard).status !== 'not-loaded') void rereadDashboard(dashboard);
}

/** The stream came back after a gap, so a dashboard event may be lost. Only a
 *  dashboard this device has read is re-read. */
export function rereadLoadedDashboards(): void {
  for (const [dashboard, view] of dashboards.value) {
    if (view.status !== 'not-loaded') void rereadDashboard(dashboard);
  }
}

/** An app changed or went away: re-read each dashboard that shows it. */
export function refreshDashboardsNaming(appId: string): void {
  for (const [dashboard, view] of dashboards.value) {
    if (view.status === 'loaded' && view.data.tiles.some((t) => t.app_id === appId)) {
      void rereadDashboard(dashboard);
    }
  }
}

/** Test-only: forget every dashboard and read. */
export function _resetDashboardsForTesting(): void {
  dashboards.value = new Map();
  newestRead.clear();
}
