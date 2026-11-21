import { describe, expect, it, vi } from 'vitest';
import { retryTransportRead, transportRetryExhausted } from './_transport';

describe('page transport retry', () => {
  it('retries one failed read', async () => {
    const request = vi.fn().mockRejectedValueOnce(new TypeError('Load failed')).mockResolvedValue('ok');
    await expect(retryTransportRead(request)).resolves.toBe('ok');
    expect(request).toHaveBeenCalledTimes(2);
  });
  it('stops after two failures', async () => {
    const request = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
    await expect(retryTransportRead(request)).rejects.toThrow('Failed to fetch');
    expect(request).toHaveBeenCalledTimes(2);
  });
  it.each([new TypeError('Load failed'), new DOMException('timeout', 'TimeoutError')])('marks a failed retry %s as exhausted', async retryError => {
    const request = vi.fn().mockRejectedValueOnce(new TypeError('Load failed')).mockRejectedValue(retryError);
    const error = await retryTransportRead(request).catch((e: unknown) => e);
    expect(error).toBe(retryError);
    expect(transportRetryExhausted(error)).toBe(true);
  });
  it('leaves a read that never retried unmarked', async () => {
    const timeout = new DOMException('timeout', 'TimeoutError');
    const error = await retryTransportRead(vi.fn().mockRejectedValue(timeout)).catch((e: unknown) => e);
    expect(transportRetryExhausted(error)).toBe(false);
  });
  it.each([new SyntaxError('bad JSON'), new TypeError('bug'), new DOMException('timeout', 'TimeoutError')])('does not retry %s', async error => {
    const request = vi.fn().mockRejectedValue(error);
    await expect(retryTransportRead(request)).rejects.toBe(error);
    expect(request).toHaveBeenCalledTimes(1);
  });
  it('does not repeat an aborted read', async () => {
    const controller = new AbortController();
    const request = vi.fn().mockImplementation(() => {
      controller.abort();
      throw new TypeError('Load failed');
    });
    await expect(retryTransportRead(request, { signal: controller.signal })).rejects.toThrow('Load failed');
    expect(request).toHaveBeenCalledTimes(1);
  });
  it('checks the generation before a retry', async () => {
    const request = vi.fn().mockRejectedValue(new TypeError('Load failed'));
    const guard = vi.fn().mockImplementationOnce(() => {}).mockImplementation(() => { throw new Error('stale'); });
    await expect(retryTransportRead(request, undefined, guard)).rejects.toThrow('stale');
    expect(request).toHaveBeenCalledTimes(1);
  });
  it('does not retry a mutation or an HTTP response', async () => {
    const request = vi.fn().mockRejectedValue(new TypeError('Load failed'));
    await expect(retryTransportRead(request, { method: 'POST' })).rejects.toThrow();
    expect(request).toHaveBeenCalledTimes(1);
    const http = vi.fn(async () => new Response('', { status: 503 }));
    expect((await retryTransportRead(http)).status).toBe(503);
    expect(http).toHaveBeenCalledTimes(1);
  });
});
