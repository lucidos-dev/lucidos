import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GatewayError, type GatewayUsersResponse, type PairedDevice } from '../../api/client/control';

const listPairedDevices = vi.fn<(gateway: string | null) => Promise<PairedDevice[]>>();
const listGatewayUsers = vi.fn<(gateway: string | null) => Promise<GatewayUsersResponse>>();

vi.mock('../../api/client/control', async (original) => ({
  ...(await original<typeof import('../../api/client/control')>()),
  listPairedDevices: (gateway: string | null) => listPairedDevices(gateway),
  listGatewayUsers: (gateway: string | null) => listGatewayUsers(gateway),
}));

describe('remote gateway lists', () => {
  beforeEach(() => {
    vi.resetModules();
    listPairedDevices.mockReset();
    listGatewayUsers.mockReset();
  });

  it('reads the named gateway, never this one', async () => {
    listPairedDevices.mockResolvedValue([]);
    const { loadRemoteGatewayDevices, remoteGatewayDevices } = await import('./remoteGatewayManage');
    await loadRemoteGatewayDevices('cloud');
    expect(listPairedDevices).toHaveBeenCalledWith('cloud');
    expect(remoteGatewayDevices.value).toEqual({ status: 'loaded', data: [] });
  });

  it('a missing grant is a settled answer, not a failure', async () => {
    listGatewayUsers.mockRejectedValue(new GatewayError('not granted', 403, 'manage-not-granted'));
    const { loadRemoteGatewayUsers, remoteGatewayUsers } = await import('./remoteGatewayManage');
    await loadRemoteGatewayUsers('cloud');
    expect(remoteGatewayUsers.value).toEqual({ status: 'loaded', data: { refused: 'not-granted' } });
  });

  it('a remote gateway too old for the hop is a settled answer', async () => {
    listPairedDevices.mockRejectedValue(new GatewayError('too old', 409, 'remote-gateway-too-old'));
    const { loadRemoteGatewayDevices, remoteGatewayDevices } = await import('./remoteGatewayManage');
    await loadRemoteGatewayDevices('cloud');
    expect(remoteGatewayDevices.value).toEqual({ status: 'loaded', data: { refused: 'too-old' } });
  });

  it('an unreachable remote gateway fails, so it never reads as an empty list', async () => {
    listPairedDevices.mockRejectedValue(new GatewayError('could not be reached', 502, 'transport'));
    const { loadRemoteGatewayDevices, remoteGatewayDevices } = await import('./remoteGatewayManage');
    await loadRemoteGatewayDevices('cloud');
    expect(remoteGatewayDevices.value.status).toBe('failed');
  });

  it('an unknown remote gateway fails rather than reading as no gateway', async () => {
    const e = new GatewayError("no remote gateway called 'cloud'", 404, 'no-remote-gateway');
    expect(e.isAbsent).toBe(false);
    listGatewayUsers.mockRejectedValue(e);
    const { loadRemoteGatewayUsers, remoteGatewayUsers } = await import('./remoteGatewayManage');
    await loadRemoteGatewayUsers('cloud');
    expect(remoteGatewayUsers.value.status).toBe('failed');
  });
});
