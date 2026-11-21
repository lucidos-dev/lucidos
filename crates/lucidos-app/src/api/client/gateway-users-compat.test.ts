import { afterEach, expect, it, vi } from 'vitest';
import {
  addGatewayUser, addGatewayUserEmail, listGatewayUsers, listPairedDevices,
  moveDeviceToGatewayUser, removeGatewayUser, removeGatewayUserEmail, renameGatewayUser,
} from './control';
import { mintPairingCode } from './pairing';

afterEach(() => vi.unstubAllGlobals());

const user = { id: 'sam', name: 'Sam', emails: [], device_ids: ['phone'] };
const legacy = {
  operators: [user],
  relayed: [{ device_id: 'remote-phone', operator_id: 'kim', name: 'Kim', via: 'cloud', emails: [] }],
};
const normalized = {
  users: [user],
  relayed: [{ device_id: 'remote-phone', user_id: 'kim', name: 'Kim', via: 'cloud', emails: [] }],
};

it('reads and normalizes an older local gateway behind a newer remote workspace', async () => {
  const fetch = vi.fn()
    .mockResolvedValueOnce(new Response(null, { status: 404 }))
    .mockResolvedValueOnce(Response.json(legacy));
  vi.stubGlobal('fetch', fetch);
  expect(await listGatewayUsers()).toEqual(normalized);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual(['/~/api/v1/auth/users', '/~/api/v1/auth/operators']);
});

it.each([
  [() => addGatewayUser(null, 'Sam'), '/operators', 'POST', { name: 'Sam' }],
  [() => renameGatewayUser(null, 'sam', 'Sam Lee'), '/operators/sam', 'PUT', { name: 'Sam Lee' }],
  [() => removeGatewayUser(null, 'sam'), '/operators/sam', 'DELETE', undefined],
  [() => addGatewayUserEmail(null, 'sam', 'sam@example.com'), '/operators/sam/emails', 'POST', { email: 'sam@example.com' }],
  [() => removeGatewayUserEmail(null, 'sam', 'sam@example.com'), '/operators/sam/emails/sam%40example.com', 'DELETE', undefined],
  [() => moveDeviceToGatewayUser(null, 'phone', 'sam'), '/devices/phone/operator', 'PUT', { operator_id: 'sam' }],
] as const)('keeps an older gateway mutation working (%s)', async (call, path, method, body) => {
  const fetch = vi.fn()
    .mockResolvedValueOnce(new Response(null, { status: 404 }))
    .mockResolvedValueOnce(Response.json(legacy));
  vi.stubGlobal('fetch', fetch);
  expect(await call()).toEqual(normalized);
  expect(fetch.mock.calls[1][0]).toBe(`/~/api/v1/auth${path}`);
  const init = fetch.mock.calls[1][1];
  expect(init.method).toBe(method);
  expect(init.body ? JSON.parse(init.body) : undefined).toEqual(body);
});

it.each([403, 500])('never retries a real gateway failure (%s)', async (status) => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ error: 'refused' }, { status }));
  vi.stubGlobal('fetch', fetch);
  await expect(listGatewayUsers()).rejects.toThrow('refused');
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('keeps a current gateway response on the current route', async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json(normalized));
  vi.stubGlobal('fetch', fetch);
  expect(await listGatewayUsers()).toEqual(normalized);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch.mock.calls[0][0]).toBe('/~/api/v1/auth/users');
});

it('does not mistake a typed local refusal for an absent route', async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ error: 'not found', reason_kind: 'user-not-found' }, { status: 404 }));
  vi.stubGlobal('fetch', fetch);
  await expect(renameGatewayUser(null, 'sam', 'Sam Lee')).rejects.toThrow('not found');
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('never sends a manage-hop refusal to the legacy local auth surface', async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ error: 'unknown gateway', reason_kind: 'no-remote-gateway' }, { status: 404 }));
  vi.stubGlobal('fetch', fetch);
  await expect(listGatewayUsers('cloud')).rejects.toThrow('unknown gateway');
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('normalizes paired device ownership from an older gateway', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json([
    { id: 'phone', label: 'My phone', paired_at: '2026-01-01T00:00:00Z', operator_id: 'sam' },
  ])));
  expect(await listPairedDevices()).toEqual([
    { id: 'phone', label: 'My phone', paired_at: '2026-01-01T00:00:00Z', user_id: 'sam' },
  ]);
});

it('mints for the chosen user on either gateway version', async () => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ code: '12345678', expires_in_secs: 300 }));
  vi.stubGlobal('fetch', fetch);
  await mintPairingCode(null, 'sam');
  const url = new URL(fetch.mock.calls[0][0], 'https://example.com');
  expect(url.searchParams.get('user')).toBe('sam');
  expect(url.searchParams.get('operator')).toBe('sam');
});
