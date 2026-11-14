import { beforeEach, describe, expect, it, vi } from 'vitest';
const m = vi.hoisted(() => ({
  confirm: vi.fn(), toast: vi.fn(), fetch: vi.fn(), generation: 'before',
}));
vi.mock('../store', () => ({ showConfirm: m.confirm, showToast: m.toast }));
vi.mock('../../utils/basePath', () => ({
  API: '/demo/api/v1', WORKSPACE_ID: 'demo@remote',
  parseWorkspaceRoute: () => ({ slug: 'demo', gateway: 'remote' }),
}));
vi.mock('../../api/client/_core', () => ({
  ApiError: class extends Error {
    constructor(readonly httpCode: number, readonly reason: string, readonly body?: unknown, readonly bootSplash = false) {
      super(reason);
    }
  },
  requestGenerationGuard: () => {
    const captured = m.generation;
    return () => { if (m.generation !== captured) throw Error('Workspace restored'); };
  },
  mutatingFetch: m.fetch,
  throwIfNotOk: async (r: Response) => { if (!r.ok) throw Error('Restore refused'); },
}));
import { confirmSnapshotRestore, restoreFailureMessage } from './restore-snapshot';
import { ApiError } from '../../api/client/_core';
const tap = { kind: 'restore-snapshot' as const, workspace_id: 'demo', handle: 'snap_00000000000000000000000000' };
describe('Undo restore confirmation', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    m.generation = 'before';
    m.confirm.mockResolvedValue(true);
    m.fetch.mockResolvedValue(new Response('{}', { status: 202 }));
  });
  it('requires confirmation naming the workspace and sends once with authoritative binding', async () => {
    await confirmSnapshotRestore(tap);
    expect(m.confirm.mock.calls[0][0]).toContain('"demo"');
    expect(m.fetch).toHaveBeenCalledTimes(1);
    expect(JSON.parse(m.fetch.mock.calls[0][1].body)).toEqual({
      expected_workspace_id: 'demo', confirmed: true,
    });
  });
  it('does nothing when canceled', async () => {
    m.confirm.mockResolvedValue(false);
    await confirmSnapshotRestore(tap);
    expect(m.fetch).not.toHaveBeenCalled();
  });
  it('refuses a different workspace before asking', async () => {
    await confirmSnapshotRestore({ ...tap, workspace_id: 'elsewhere' });
    expect(m.confirm).not.toHaveBeenCalled();
    expect(m.fetch).not.toHaveBeenCalled();
    expect(m.toast).toHaveBeenCalled();
  });
  it('does not relabel intent when generation changes during confirmation', async () => {
    m.confirm.mockImplementation(async () => { m.generation = 'after'; return true; });
    await confirmSnapshotRestore(tap);
    expect(m.fetch).not.toHaveBeenCalled();
    expect(m.toast).toHaveBeenCalled();
  });
  // Only a request that may have started a restore says its delivery is
  // unknown. A refusal the engine answered names its reason instead.
  it.each([
    [new TypeError('Failed to fetch'), true],
    [new ApiError(500, 'snapshot_restore_delivery_unknown'), true],
    [new ApiError(503, 'Engine unavailable', undefined, true), true],
    [new ApiError(403, 'snapshot_restore_policy_owner_device_required'), false],
    [new ApiError(503, 'snapshot_restore_not_delivered'), false],
    [new ApiError(409, 'Workspace restored; review retained work before trying again'), false],
  ])('classifies %s as unknown delivery: %s', (error, unknown) => {
    expect(restoreFailureMessage(error).includes('Check restore status')).toBe(unknown);
  });
  it('does not retry an unknown delivery', async () => {
    m.fetch.mockRejectedValue(new TypeError('Failed to fetch'));
    await confirmSnapshotRestore(tap);
    expect(m.fetch).toHaveBeenCalledTimes(1);
    expect(m.toast.mock.calls[0][0]).toContain('Check restore status');
  });
});
