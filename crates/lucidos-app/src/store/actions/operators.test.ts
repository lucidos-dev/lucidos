import { beforeEach, describe, expect, it, vi } from 'vitest';
import { GatewayError, type OperatorsResponse } from '../../api/client/control';

const listOperators = vi.fn<() => Promise<OperatorsResponse>>();
const moveDeviceToOperator = vi.fn<() => Promise<OperatorsResponse>>();
const showToast = vi.fn();

vi.mock('../../api/client/control', async (original) => ({
  ...(await original<typeof import('../../api/client/control')>()),
  listOperators: () => listOperators(),
  moveDeviceToOperator: () => moveDeviceToOperator(),
}));
vi.mock('../store', () => ({
  showConfirm: vi.fn(),
  showPrompt: vi.fn(),
  showToast: (...args: unknown[]) => showToast(...args),
}));

const person = (id: string, devices: string[] = []) => ({
  id,
  name: id,
  emails: [],
  device_ids: devices,
});

describe('operators', () => {
  beforeEach(() => {
    vi.resetModules();
    listOperators.mockReset();
    moveDeviceToOperator.mockReset();
    showToast.mockReset();
  });

  it('a gateway with one person is not a team, so no team control draws', async () => {
    listOperators.mockResolvedValue({ operators: [person('Owner', ['d1'])], relayed: [] });
    const { loadOperators, isTeamGateway } = await import('./operators');
    await loadOperators();
    expect(isTeamGateway.value).toBe(false);
  });

  it('two people make a team', async () => {
    listOperators.mockResolvedValue({ operators: [person('Owner'), person('Sam')], relayed: [] });
    const { loadOperators, isTeamGateway } = await import('./operators');
    await loadOperators();
    expect(isTeamGateway.value).toBe(true);
  });

  it('a page with no gateway loads as no gateway, never as a failure', async () => {
    listOperators.mockRejectedValue(new GatewayError('not found', 404));
    const { loadOperators, operators } = await import('./operators');
    const { NO_GATEWAY } = await import('./pairedDevices');
    await loadOperators();
    expect(operators.value).toEqual({ status: 'loaded', data: NO_GATEWAY });
  });

  it('a broken gateway is a failure, never an empty list', async () => {
    listOperators.mockRejectedValue(new GatewayError('boom', 500));
    const { loadOperators, operators } = await import('./operators');
    await loadOperators();
    expect(operators.value.status).toBe('failed');
  });

  it('a refused change says so in a toast and keeps the list', async () => {
    listOperators.mockResolvedValue({ operators: [person('Owner', ['d1'])], relayed: [] });
    moveDeviceToOperator.mockRejectedValue(new GatewayError('no person with that id', 400));
    const { loadOperators, moveDevice, operatorRows, operators } = await import('./operators');
    await loadOperators();
    await moveDevice('d1', 'nobody');
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('no person with that id'), 'error');
    expect(operatorRows(operators.value)?.[0].device_ids).toEqual(['d1']);
  });

  describe('operatorName', () => {
    it('names the holder on a gateway with one operator', async () => {
      const { operatorName } = await import('./operators');
      const solo = { status: 'loaded' as const, data: { operators: [person('Owner', ['mine'])], relayed: [] } };
      expect(operatorName(solo, 'mine')).toBe('Owner');
      expect(operatorName(solo, 'unknown')).toBeNull();
    });

    it('returns null without a loaded operator list', async () => {
      const { operatorName } = await import('./operators');
      const { NO_GATEWAY } = await import('./pairedDevices');
      expect(operatorName({ status: 'not-loaded' }, 'mine')).toBeNull();
      expect(operatorName({ status: 'loading' }, 'mine')).toBeNull();
      expect(operatorName({ status: 'failed', error: 'x' }, 'mine')).toBeNull();
      expect(operatorName({ status: 'loaded', data: NO_GATEWAY }, 'mine')).toBeNull();
    });
  });

  describe('senderName', () => {
    const loaded = (data: OperatorsResponse) => ({ status: 'loaded' as const, data });
    const team = loaded({
      operators: [person('Owner', ['mine', 'mine-phone']), person('Sam', ['sams'])],
      relayed: [{ device_id: 'far', operator_id: 'o9', name: 'Kim', via: 'A', emails: [] }],
    });

    it('is null for the reader and for a second device of the same person', async () => {
      const { senderName } = await import('./operators');
      expect(senderName(team, 'mine', 'mine')).toBeNull();
      expect(senderName(team, 'mine', 'mine-phone')).toBeNull();
    });

    it('names a teammate from the operator list', async () => {
      const { senderName } = await import('./operators');
      expect(senderName(team, 'mine', 'sams')).toBe('Sam');
    });

    it('names a relayed person with the gateway it came through', async () => {
      const { senderName } = await import('./operators');
      expect(senderName(team, 'mine', 'far')).toBe('Kim via A');
    });

    it('says Unknown person for a device no operator holds', async () => {
      const { senderName, UNKNOWN_PERSON } = await import('./operators');
      expect(senderName(team, 'mine', 'gone')).toBe(UNKNOWN_PERSON);
    });

    it('draws as today on a gateway with one person and no relayed device', async () => {
      const { senderName } = await import('./operators');
      const solo = loaded({ operators: [person('Owner', ['mine', 'other'])], relayed: [] });
      expect(senderName(solo, 'mine', 'other')).toBeNull();
      expect(senderName(solo, 'mine', 'gone')).toBeNull();
    });

    it('treats a relayed reader as a team: a local device is a teammate', async () => {
      const { senderName } = await import('./operators');
      const relayedReader = loaded({
        operators: [person('Owner', ['local'])],
        relayed: [{ device_id: 'me', operator_id: 'o1', name: 'Me', via: 'A', emails: [] }],
      });
      expect(senderName(relayedReader, 'me', 'local')).toBe('Owner');
    });

    it('never invents a teammate while the list loads, fails, or has no gateway', async () => {
      const { senderName } = await import('./operators');
      const { NO_GATEWAY } = await import('./pairedDevices');
      expect(senderName({ status: 'not-loaded' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'loading' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'failed', error: 'x' }, 'mine', 'sams')).toBeNull();
      expect(senderName({ status: 'loaded', data: NO_GATEWAY }, 'mine', 'sams')).toBeNull();
    });

    it('is null when the reader holds no device in the list', async () => {
      const { senderName } = await import('./operators');
      expect(senderName(team, 'stranger', 'sams')).toBeNull();
    });
  });

  describe('personName', () => {
    const loaded = (data: OperatorsResponse) => ({ status: 'loaded' as const, data });
    const team = loaded({
      operators: [person('Owner', ['mine']), person('Sam', ['sams'])],
      relayed: [{ device_id: 'far', operator_id: 'o9', name: 'Kim', via: 'A', emails: [] }],
    });

    it('names any device on a team gateway, the reader\'s own included', async () => {
      const { personName, UNKNOWN_PERSON } = await import('./operators');
      expect(personName(team, 'mine')).toBe('Owner');
      expect(personName(team, 'sams')).toBe('Sam');
      expect(personName(team, 'far')).toBe('Kim via A');
      expect(personName(team, 'gone')).toBe(UNKNOWN_PERSON);
    });

    it('is null with nobody to tell apart, and while the list is not loaded', async () => {
      const { personName } = await import('./operators');
      expect(personName(loaded({ operators: [person('Owner', ['mine'])], relayed: [] }), 'mine')).toBeNull();
      expect(personName({ status: 'loading' }, 'mine')).toBeNull();
    });
  });
});
