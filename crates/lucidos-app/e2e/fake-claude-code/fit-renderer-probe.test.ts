import { describe, expect, it } from 'vitest';
import { fitRendererBase, fitReportHeaders, probeFitRenderer } from './fit-renderer-probe.mjs';

describe('the fit-only launcher channel', () => {
  const frame = {};
  const base = new URL('http://localhost:8123');
  const request = (overrides = {}) => ({
    url: () => `${base.origin}/api/v1/widgets/fit-report`, frame: () => frame,
    method: () => 'POST', postDataJSON: () => ({ device_id: 'renderer' }), headers: () => ({}), ...overrides,
  });
  it('attaches the secret only to the exact top-frame report at the engine origin', () => {
    expect(fitReportHeaders(request(), frame, base, 'renderer', 'test-secret')).toEqual({ 'x-lucidos-fit-renderer': 'test-secret' });
    for (const overrides of [
      { frame: () => ({}) }, { method: () => 'GET' },
      { url: () => 'http://localhost:8124/api/v1/widgets/fit-report' },
      { url: () => `${base.origin}/api/v1/widgets/fit-report?copy=1` },
      { url: () => `${base.origin}/api/v1/events` },
      { postDataJSON: () => ({ device_id: 'other-device' }) },
    ]) expect(fitReportHeaders(request(overrides), frame, base, 'renderer', 'test-secret')).toBeNull();
  });
  it('refuses absent provenance without starting a browser or printing credentials', async () => {
    expect(await probeFitRenderer([], {})).toEqual({ verified: false, stage: 'origin' });
  });
  it('resolves gateway and direct engine addresses without guessing a port', () => {
    expect(fitRendererBase({ LUCIDOS_API_BASE_URL: 'http://localhost:8123/path' }).href).toBe('http://localhost:8123/path/');
    expect(fitRendererBase({ LUCIDOS_API_PORT: '8123' }).href).toBe('http://127.0.0.1:8123/');
    expect(fitRendererBase({ LUCIDOS_API_PORT: '8123', LUCIDOS_TLS_CERT: 'cert', LUCIDOS_TLS_KEY: 'key' }).protocol).toBe('https:');
    for (const env of [{}, { LUCIDOS_API_PORT: '0' }, { LUCIDOS_API_PORT: '65536' }, { LUCIDOS_API_BASE_URL: 'https://example.com' }, { LUCIDOS_API_BASE_URL: 'http://secret@localhost' }]) {
      expect(() => fitRendererBase(env)).toThrow();
    }
  });
});
