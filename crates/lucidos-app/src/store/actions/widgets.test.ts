import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { ThreadWidget } from '../../api/client/widgets';

const fetchThreadWidgets = vi.fn();
vi.mock('../../api/client/widgets', () => ({
  fetchThreadWidgets: (...args: unknown[]) => fetchThreadWidgets(...args),
  getAppApi: vi.fn(),
  listReusableWidgetsApi: vi.fn(),
  makeWidgetReusableApi: vi.fn(),
  pinWidgetApi: vi.fn(),
  stopReusingWidgetApi: vi.fn(),
  unpinWidgetApi: vi.fn(),
}));

const { appliedThreadWidgetTicket, threadWidgets, threadWidgetsFor } = await import('../widgets');
const { forgetThreadWidgets, refreshThreadWidgetsNaming, rereadThreadWidgets } = await import('./widgets');

function widget(appId: string): ThreadWidget {
  return { app_id: appId, name: appId, reusable: false, reveal: 'on-load', pinned: false, shown_event_id: `e-${appId}` };
}

/** A fetch the test settles by hand, in any order. */
function deferred() {
  let resolve!: (v: ThreadWidget[]) => void;
  const promise = new Promise<ThreadWidget[]>((r) => { resolve = r; });
  return { promise, resolve };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('the thread widgets store (ADRs 0402, 0407)', () => {
  beforeEach(() => {
    fetchThreadWidgets.mockReset();
    threadWidgets.value = new Map();
  });

  it('applies only the newest read, so a late older answer cannot put back a removed chip', async () => {
    const older = deferred();
    const newer = deferred();
    fetchThreadWidgets.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);

    rereadThreadWidgets('t1');
    const second = rereadThreadWidgets('t1');
    newer.resolve([widget('currency')]);
    await flush();
    older.resolve([widget('fare-grid'), widget('currency')]);
    await flush();

    const widgets = threadWidgetsFor('t1');
    expect(widgets.status === 'loaded' && widgets.data.map((w) => w.app_id)).toEqual(['currency']);
    expect(appliedThreadWidgetTicket('t1')).toBe(second);
  });

  it('re-reads only the threads whose widgets include a changed app', async () => {
    threadWidgets.value = new Map([
      ['names-it', { status: 'loaded', data: [widget('fare-grid')] }],
      ['does-not', { status: 'loaded', data: [widget('currency')] }],
    ]);
    fetchThreadWidgets.mockResolvedValue([]);
    refreshThreadWidgetsNaming('fare-grid');
    await flush();
    expect(fetchThreadWidgets).toHaveBeenCalledTimes(1);
    expect(fetchThreadWidgets).toHaveBeenCalledWith('names-it');
  });

  it('forgets a deleted thread\'s widgets', () => {
    threadWidgets.value = new Map([['gone', { status: 'loaded', data: [widget('fare-grid')] }]]);
    forgetThreadWidgets(['gone']);
    expect(threadWidgetsFor('gone').status).toBe('not-loaded');
  });
});
