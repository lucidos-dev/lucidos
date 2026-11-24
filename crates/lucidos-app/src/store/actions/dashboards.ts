
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

type TileNoun = DashboardTile['kind'];

function setDashboard(dashboard: Dashboard, value: ReturnType<typeof dashboardFor>): void {
  const next = new Map(dashboards.value);
  next.set(dashboard, value);
  dashboards.value = next;
}

const newestRead = new Map<Dashboard, number>();
let lastTicket = 0;

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
      if (dashboardFor(dashboard).status !== 'loaded') setDashboard(dashboard, toFailed(error));
    },
  );
}

export function toArranged(tiles: readonly DashboardTile[]): ArrangedTile[] {
  return tiles.map((t) => ({
    app_id: t.app_id,
    ...(t.kind === 'widget' && t.params ? { params: t.params } : {}),
    size: t.size,
  }));
}

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

export async function removeFromDashboard(dashboard: Dashboard, tile: DashboardTileRef, name: string, noun: TileNoun): Promise<void> {
  try {
    await removeFromDashboardApi(dashboard, tile);
  } catch (error) {
    showToast(`Couldn't remove ${noun} "${name}" from the ${dashboard} dashboard: ${errorDetail(error)}`, 'error');
  }
}

export function onDashboardEvent(dashboard: Dashboard): void {
  if (dashboardFor(dashboard).status !== 'not-loaded') void rereadDashboard(dashboard);
}

export function rereadLoadedDashboards(): void {
  for (const [dashboard, view] of dashboards.value) {
    if (view.status !== 'not-loaded') void rereadDashboard(dashboard);
  }
}

export function refreshDashboardsNaming(appId: string): void {
  for (const [dashboard, view] of dashboards.value) {
    if (view.status === 'loaded' && view.data.tiles.some((t) => t.app_id === appId)) {
      void rereadDashboard(dashboard);
    }
  }
}

export function _resetDashboardsForTesting(): void {
  dashboards.value = new Map();
  newestRead.clear();
}
