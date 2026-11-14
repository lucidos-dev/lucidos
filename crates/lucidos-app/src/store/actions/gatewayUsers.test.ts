import { REPO_ROOT } from '../../styles/__tests__/css-rule-helpers';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { resolve } from 'node:path';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GatewayError, type GatewayUsersResponse } from '../../api/client/control';

const listGatewayUsers = vi.fn<() => Promise<GatewayUsersResponse>>();
const moveDeviceToGatewayUser = vi.fn<(...args: unknown[]) => Promise<GatewayUsersResponse>>();
const showToast = vi.fn();

vi.mock('../../api/client/control', async (original) => ({
  ...(await original<typeof import('../../api/client/control')>()),
  listGatewayUsers: () => listGatewayUsers(),
  moveDeviceToGatewayUser: (...args: unknown[]) => moveDeviceToGatewayUser(...args),
}));
vi.mock('../store', () => ({
  showConfirm: vi.fn(),
  showPrompt: vi.fn(),
  showToast: (...args: unknown[]) => showToast(...args),
}));

const user = (id: string, devices: string[] = []) => ({
  id,
  name: id,
  emails: [],
  device_ids: devices,
});

describe('users', () => {
  beforeEach(() => {
    vi.resetModules();
    listGatewayUsers.mockReset();
    moveDeviceToGatewayUser.mockReset();
    showToast.mockReset();
  });

  it('a gateway with one user is not a team, so no team control draws', async () => {
    listGatewayUsers.mockResolvedValue({ users: [user('Owner', ['d1'])], relayed: [] });
    const { loadGatewayUsers, isTeamGateway } = await import('./gatewayUsers');
    await loadGatewayUsers();
    expect(isTeamGateway.value).toBe(false);
  });

  it('two users make a team', async () => {
    listGatewayUsers.mockResolvedValue({ users: [user('Owner'), user('Sam')], relayed: [] });
    const { loadGatewayUsers, isTeamGateway } = await import('./gatewayUsers');
    await loadGatewayUsers();
    expect(isTeamGateway.value).toBe(true);
  });

  it('a page with no gateway loads as no gateway, never as a failure', async () => {
    listGatewayUsers.mockRejectedValue(new GatewayError('not found', 404));
    const { loadGatewayUsers, gatewayUsers } = await import('./gatewayUsers');
    const { NO_GATEWAY } = await import('./pairedDevices');
    await loadGatewayUsers();
    expect(gatewayUsers.value).toEqual({ status: 'loaded', data: NO_GATEWAY });
  });

  it('a broken gateway is a failure, never an empty list', async () => {
    listGatewayUsers.mockRejectedValue(new GatewayError('boom', 500));
    const { loadGatewayUsers, gatewayUsers } = await import('./gatewayUsers');
    await loadGatewayUsers();
    expect(gatewayUsers.value.status).toBe('failed');
  });

  it('a refused change says so in a toast and keeps the list', async () => {
    listGatewayUsers.mockResolvedValue({ users: [user('Owner', ['d1'])], relayed: [] });
    moveDeviceToGatewayUser.mockRejectedValue(new GatewayError('no user with that id', 400));
    const { loadGatewayUsers, gatewayUserRows, gatewayUsers } = await import('./gatewayUsers');
    const { moveDevice } = await import('./gatewayUserManagement');
    await loadGatewayUsers();
    await moveDevice(null, 'd1', 'nobody');
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('no user with that id'), 'error');
    expect(gatewayUserRows(gatewayUsers.value)?.[0].device_ids).toEqual(['d1']);
  });

  it("a remote gateway's change lands in its own list and leaves this gateway's alone", async () => {
    listGatewayUsers.mockResolvedValue({ users: [user('Owner', ['d1'])], relayed: [] });
    const remote = { users: [user('Sam', ['b1']), user('Kim', [])], relayed: [] };
    moveDeviceToGatewayUser.mockResolvedValue(remote);
    const { loadGatewayUsers, gatewayUserRows, gatewayUsers } = await import('./gatewayUsers');
    const { moveDevice } = await import('./gatewayUserManagement');
    const { remoteGatewayUsers } = await import('./remoteGatewayManage');
    await loadGatewayUsers();
    await moveDevice('cloud', 'b1', 'Sam');
    expect(moveDeviceToGatewayUser).toHaveBeenCalledWith('cloud', 'b1', 'Sam');
    expect(remoteGatewayUsers.value).toEqual({ status: 'loaded', data: remote });
    expect(gatewayUserRows(gatewayUsers.value)?.map((u) => u.name)).toEqual(['Owner']);
  });

  describe('gatewayUserName', () => {
    it('names the holder on a gateway with one user', async () => {
      const { gatewayUserName } = await import('./gatewayUsers');
      const solo = { status: 'loaded' as const, data: { users: [user('Owner', ['mine'])], relayed: [] } };
      expect(gatewayUserName(solo, 'mine')).toBe('Owner');
      expect(gatewayUserName(solo, 'unknown')).toBeNull();
    });

    it('returns null without a loaded user list', async () => {
      const { gatewayUserName } = await import('./gatewayUsers');
      const { NO_GATEWAY } = await import('./pairedDevices');
      expect(gatewayUserName({ status: 'not-loaded' }, 'mine')).toBeNull();
      expect(gatewayUserName({ status: 'loading' }, 'mine')).toBeNull();
      expect(gatewayUserName({ status: 'failed', error: 'x' }, 'mine')).toBeNull();
      expect(gatewayUserName({ status: 'loaded', data: NO_GATEWAY }, 'mine')).toBeNull();
    });
  });

  describe('senderName', () => {
    const loaded = (data: GatewayUsersResponse) => ({ status: 'loaded' as const, data });
    const team = loaded({
      users: [user('Owner', ['mine', 'mine-phone']), user('Sam', ['sams'])],
      relayed: [{ device_id: 'far', user_id: 'o9', name: 'Kim', via: 'A', emails: [] }],
    });

    it('is null for the reader and for a second device of the same user', async () => {
      const { senderName } = await import('./gatewayUsers');
      expect(senderName(team, 'mine', 'mine')).toBeNull();
      expect(senderName(team, 'mine', 'mine-phone')).toBeNull();
    });

    it('names a teammate from the user list', async () => {
      const { senderName } = await import('./gatewayUsers');
      expect(senderName(team, 'mine', 'sams')).toBe('Sam');
    });

    it('names a relayed user with the gateway it came through', async () => {
      const { senderName } = await import('./gatewayUsers');
      expect(senderName(team, 'mine', 'far')).toBe('Kim');
    });

    it('says Unknown user for a device no user holds', async () => {
      const { senderName, UNKNOWN_USER } = await import('./gatewayUsers');
      expect(senderName(team, 'mine', 'gone')).toBe(UNKNOWN_USER);
    });

    it('draws as today on a gateway with one user and no relayed device', async () => {
      const { senderName } = await import('./gatewayUsers');
      const solo = loaded({ users: [user('Owner', ['mine', 'other'])], relayed: [] });
      expect(senderName(solo, 'mine', 'other')).toBeNull();
      expect(senderName(solo, 'mine', 'gone')).toBeNull();
    });

    it('treats a relayed reader as a team: a local device is a teammate', async () => {
      const { senderName } = await import('./gatewayUsers');
      const relayedReader = loaded({
        users: [user('Owner', ['local'])],
        relayed: [{ device_id: 'me', user_id: 'o1', name: 'Me', via: 'A', emails: [] }],
      });
      expect(senderName(relayedReader, 'me', 'local')).toBe('Owner');
    });

    it('never invents a teammate while the list loads, fails, or has no gateway', async () => {
      const { senderName } = await import('./gatewayUsers');
      const { NO_GATEWAY } = await import('./pairedDevices');
      expect(senderName({ status: 'not-loaded' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'loading' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'failed', error: 'x' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'loaded', data: NO_GATEWAY }, 'mine', 'sams')).toBeNull();
    });

    it('is null when the reader holds no device in the list', async () => {
      const { senderName } = await import('./gatewayUsers');
      expect(senderName(team, 'stranger', 'sams')).toBeNull();
    });
  });

  describe('userName', () => {
    const loaded = (data: GatewayUsersResponse) => ({ status: 'loaded' as const, data });
    const team = loaded({
      users: [user('Owner', ['mine']), user('Sam', ['sams'])],
      relayed: [{ device_id: 'far', user_id: 'o9', name: 'Kim', via: 'A', emails: [] }],
    });

    it('names any device on a team gateway, the reader\'s own included', async () => {
      const { userName, UNKNOWN_USER } = await import('./gatewayUsers');
      expect(userName(team, 'mine')).toBe('Owner');
      expect(userName(team, 'sams')).toBe('Sam');
      expect(userName(team, 'far')).toBe('Kim');
      expect(userName(team, 'gone')).toBe(UNKNOWN_USER);
    });

    it('is null with nobody to tell apart, and while the list is not loaded', async () => {
      const { userName } = await import('./gatewayUsers');
      expect(userName(loaded({ users: [user('Owner', ['mine'])], relayed: [] }), 'mine')).toBeNull();
      expect(userName({ status: 'loading' }, 'mine')).toBeNull();
    });
  });
  describe('relayed names', () => {
    it('follow the fixture the engine tests read too', async () => {
      const { gatewayUserName } = await import('./gatewayUsers');
      const path = resolve(REPO_ROOT, 'crates/lucidos-engine/src/gateway_user_names.json');
      const cases: { case: string; snapshot: GatewayUsersResponse; names: Record<string, string> }[] = JSON.parse(
        readFileSync(path, 'utf8'),
      );
      expect(cases.length).toBeGreaterThan(0);
      for (const { case: label, snapshot, names } of cases) {
        for (const [deviceId, expected] of Object.entries(names)) {
          expect(gatewayUserName({ status: 'loaded', data: snapshot }, deviceId), `${label}: ${deviceId}`).toBe(expected);
        }
      }
    });

    it('count a relayed user merged by email as the local user', async () => {
      const { senderName } = await import('./gatewayUsers');
      const data: GatewayUsersResponse = {
        users: [{ ...user('Sam', ['sams']), emails: ['sam@example.com'] }, user('Alex', ['alexs'])],
        relayed: [{ device_id: 'laptop', user_id: 'o9', name: 'Laptop Sam', via: 'A', emails: ['SAM@example.com'] }],
      };
      expect(senderName({ status: 'loaded', data }, 'sams', 'laptop')).toBeNull();
      expect(senderName({ status: 'loaded', data }, 'alexs', 'laptop')).toBe('Sam');
    });
  });
});
