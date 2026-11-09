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
});
