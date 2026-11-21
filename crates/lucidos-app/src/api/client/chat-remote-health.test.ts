import { afterEach, expect, it, vi } from 'vitest';
import { checkHealth } from './chat';
import { remoteConnectionFailure } from '../../utils/remoteConnection';

afterEach(() => { vi.unstubAllGlobals(); remoteConnectionFailure.value = null; });
it('recognizes a typed remote relay failure and clears it on recovery', async () => {
  vi.stubGlobal('fetch', vi.fn()
    .mockResolvedValueOnce(new Response(JSON.stringify({ remote_gateway: 'cloud', reason_kind: 'timeout', reason: 'Remote timed out' }), { status: 502 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ status: 'ok' }), { status: 200 })));
  expect((await checkHealth()).status).toBe('failed');
  expect(remoteConnectionFailure.value).toEqual({ reason: 'Remote timed out', reason_kind: 'timeout' });
  expect((await checkHealth()).status).toBe('loaded');
  expect(remoteConnectionFailure.value).toBeNull();
});
it('drops the old remote failure when the next probe fails for another reason', async () => {
  remoteConnectionFailure.value = { reason: 'Old timeout', reason_kind: 'timeout' };
  vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Network offline')));
  expect((await checkHealth()).status).toBe('failed');
  expect(remoteConnectionFailure.value).toBeNull();
});
