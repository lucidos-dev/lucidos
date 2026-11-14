import { beforeEach, describe, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({ confirm: vi.fn(), toast: vi.fn(), fetch: vi.fn() }));
vi.mock('../store', () => ({ showConfirm: m.confirm, showToast: m.toast }));
vi.mock('../../utils/basePath', () => ({ API: '/demo/api/v1' }));
vi.mock('../../api/client/_core', () => ({
  mutatingFetch: m.fetch,
  throwIfNotOk: async (r: Response) => { if (!r.ok) throw Error('The config has changed since'); },
}));
import { confirmConfigUndo } from './undo-config-change';

const tap = { kind: 'undo-config-change' as const, event_id: '0b3f2c1e-8d4a-4c6e-9f10-2a3b4c5d6e7f' };

describe('config change Undo', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    m.confirm.mockResolvedValue(true);
    m.fetch.mockResolvedValue(new Response('{}', { status: 200 }));
  });
  it('asks first, then posts the undo for that event once', async () => {
    await confirmConfigUndo(tap);
    expect(m.confirm).toHaveBeenCalledTimes(1);
    expect(m.fetch).toHaveBeenCalledTimes(1);
    expect(m.fetch.mock.calls[0][0]).toBe(`/demo/api/v1/config-changes/${tap.event_id}/undo`);
    expect(m.toast.mock.calls[0][1]).toBe('success');
  });
  it('does nothing when canceled', async () => {
    m.confirm.mockResolvedValue(false);
    await confirmConfigUndo(tap);
    expect(m.fetch).not.toHaveBeenCalled();
  });
  it('tells the user when the engine refuses', async () => {
    m.fetch.mockResolvedValue(new Response('{}', { status: 409 }));
    await confirmConfigUndo(tap);
    expect(m.toast.mock.calls[0][1]).toBe('error');
  });
});
