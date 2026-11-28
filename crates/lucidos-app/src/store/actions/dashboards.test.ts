import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DashboardView } from '../../api/client/widgets';
import { dashboardFor, dashboards } from '../dashboards';

const fetchDashboardMock = vi.fn();
vi.mock('../../api/client/widgets', () => ({
  fetchDashboard: (...args: unknown[]) => fetchDashboardMock(...args),
  addToDashboardApi: vi.fn(),
  arrangeDashboardApi: vi.fn(),
  removeFromDashboardApi: vi.fn(),
}));
vi.mock('../store', () => ({ showToast: vi.fn() }));

const { rereadDashboard, rereadLoadedDashboards, _resetDashboardsForTesting } = await import('./dashboards');

function view(dashboard: 'desktop' | 'phone', appId: string): DashboardView {
  return { dashboard, tiles: [{ kind: 'app', app_id: appId, name: appId, size: 'quarter' }] } as DashboardView;
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

describe('rereadDashboard', () => {
  beforeEach(() => {
    _resetDashboardsForTesting();
    fetchDashboardMock.mockReset();
  });

  it('applies only the newest read, so a late older answer cannot put back a tile a newer read removed', async () => {
    const older = deferred<DashboardView>();
    const newer = deferred<DashboardView>();
    fetchDashboardMock.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);

    const olderRead = rereadDashboard('desktop');
    const newerRead = rereadDashboard('desktop');
    newer.resolve(view('desktop', 'notes'));
    await newerRead;
    older.resolve(view('desktop', 'removed-tile'));
    await olderRead;

    const shown = dashboardFor('desktop');
    expect(shown.status === 'loaded' && shown.data.tiles[0].app_id).toBe('notes');
  });
});

describe('rereadLoadedDashboards', () => {
  beforeEach(() => {
    _resetDashboardsForTesting();
    fetchDashboardMock.mockReset();
  });

  it('re-reads each dashboard this device has loaded, and no dashboard it has not read', async () => {
    dashboards.value = new Map<'desktop' | 'phone', ReturnType<typeof dashboardFor>>([
      ['desktop', { status: 'loaded', data: view('desktop', 'notes') }],
      ['phone', { status: 'not-loaded' }],
    ]);
    fetchDashboardMock.mockResolvedValue(view('desktop', 'notes'));

    rereadLoadedDashboards();

    expect(fetchDashboardMock).toHaveBeenCalledTimes(1);
    expect(fetchDashboardMock).toHaveBeenCalledWith('desktop');
  });
});
