import { describe, it, expect, vi, beforeEach } from 'vitest';

const listReusableWidgetsApi = vi.fn();
vi.mock('../../api/client/widgets', () => ({
  listReusableWidgetsApi: (...args: unknown[]) => listReusableWidgetsApi(...args),
}));

const { reusableWidgets } = await import('../widgets');
const { refreshReusableWidgets } = await import('./widgets');

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('the reusable widgets list (ADR 0402)', () => {
  beforeEach(() => {
    listReusableWidgetsApi.mockReset();
    listReusableWidgetsApi.mockResolvedValue([]);
  });

  it('re-reads on an app change once this device has read it', async () => {
    reusableWidgets.value = { status: 'loaded', data: [] };
    refreshReusableWidgets();
    await flush();
    expect(listReusableWidgetsApi).toHaveBeenCalledTimes(1);
  });

  it('reads nothing for a list this device never opened', async () => {
    reusableWidgets.value = { status: 'not-loaded' };
    refreshReusableWidgets();
    await flush();
    expect(listReusableWidgetsApi).not.toHaveBeenCalled();
  });
});
