import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../../../api/client/_core';
import {
  GatewayError,
  clearRestoreStatus,
  createRemoteWorkspace,
  getRestoreStatus,
  remoteControlBase,
  startRemoteWorkspace,
  stopRemoteWorkspace,
} from '../../../api/client/control';
import { hopRefusalReason, remoteControlBlocker, remoteControlReason } from '../remoteControl';

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function answer(status: number, body: unknown = null) {
  const fetch = vi.fn(async () => new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  }));
  globalThis.fetch = fetch as unknown as typeof globalThis.fetch;
  return fetch;
}

describe('a refused hop call says why in plain words', () => {
  it('names the missing control grant and where B turns it on', () => {
    const reason = hopRefusalReason(new GatewayError('not granted', 403, 'control-not-granted'), 'lab');
    expect(reason).toContain('lab has not allowed this gateway');
    expect(reason).toContain('Settings → Access → Pair a gateway');
  });

  it('names an old gateway', () => {
    expect(hopRefusalReason(new GatewayError('old', 409, 'remote-gateway-too-old'), 'lab'))
      .toBe('lab needs an update before this gateway can control its workspaces.');
  });

  it('names a gateway that dropped the pairing, and one that is silent', () => {
    expect(hopRefusalReason(new GatewayError('refused', 502, 'remote-refused'), 'lab')).toContain('Pair it again');
    expect(hopRefusalReason(new GatewayError('timed out', 502, 'transport'), 'lab')).toBe('lab is not answering.');
    expect(hopRefusalReason(new GatewayError('offline', 0), 'lab')).toBe('lab is not answering.');
  });

  it('reads the reason kind from an upload refusal too', () => {
    const err = new ApiError(403, 'not granted', { error: 'not granted', reason_kind: 'control-not-granted' });
    expect(hopRefusalReason(err, 'lab')).toContain('has not allowed');
  });

  it('leaves any other failure to its own message', () => {
    expect(hopRefusalReason(new GatewayError('name taken', 409, null), 'lab')).toBeNull();
    expect(remoteControlReason(new GatewayError('name taken', 409, null), 'lab')).toBe('name taken');
  });
});

describe('the control probe', () => {
  it('lists B through the hop and finds nothing in the way', async () => {
    const fetch = answer(200, []);
    expect(await remoteControlBlocker('lab')).toBeNull();
    expect((fetch.mock.calls[0] as unknown as [string])[0]).toBe('/~/api/v1/remote-gateways/lab/control/workspaces');
  });

  it('turns a refusal into the reason the actions show', async () => {
    answer(403, { error: 'no', reason_kind: 'control-not-granted' });
    expect(await remoteControlBlocker('lab')).toContain('has not allowed');
  });
});

describe('hop client calls', () => {
  it('creates on B and returns its listed row', async () => {
    const fetch = answer(201, { workspace: { id: 'notes', name: 'notes', health: 'booting' } });
    expect(await createRemoteWorkspace('lab', 'notes')).toEqual({ id: 'notes', name: 'notes', health: 'booting' });
    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/~/api/v1/remote-gateways/lab/control/workspaces');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({ name: 'notes' });
  });

  it('starts and stops on B, accepting an empty 202', async () => {
    const fetch = answer(202);
    await startRemoteWorkspace('lab', 'notes');
    await stopRemoteWorkspace('lab', 'notes');
    expect(fetch.mock.calls.map((call) => (call as unknown as [string])[0])).toEqual([
      '/~/api/v1/remote-gateways/lab/control/workspaces/notes/start',
      '/~/api/v1/remote-gateways/lab/control/workspaces/notes/stop',
    ]);
  });

  it('reads the restore status on the gateway named by the base', async () => {
    const fetch = answer(200, { status: 'idle' });
    await getRestoreStatus(remoteControlBase('lab'));
    await getRestoreStatus();
    expect(fetch.mock.calls.map((call) => (call as unknown as [string])[0])).toEqual([
      '/~/api/v1/remote-gateways/lab/control/restore-status',
      '/~/api/v1/control/restore-status',
    ]);
  });

  it('dismisses the displayed result on the gateway that returned it', async () => {
    const fetch = answer(204);
    await clearRestoreStatus('result-1', remoteControlBase('lab'));
    await clearRestoreStatus(undefined);
    const calls = fetch.mock.calls as unknown as [string, RequestInit][];
    expect(calls.map(([url, init]) => [url, init.method])).toEqual([
      ['/~/api/v1/remote-gateways/lab/control/restore-status?result_id=result-1', 'DELETE'],
      ['/~/api/v1/control/restore-status', 'DELETE'],
    ]);
  });

  it('keeps the reason kind of a refused start', async () => {
    answer(409, { error: 'too old', reason_kind: 'remote-gateway-too-old' });
    await expect(startRemoteWorkspace('lab', 'notes')).rejects.toMatchObject({
      status: 409,
      reasonKind: 'remote-gateway-too-old',
    });
  });
});
