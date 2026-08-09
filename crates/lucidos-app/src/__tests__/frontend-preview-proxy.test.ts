import { describe, it, expect } from 'vitest';
import {
  frontendPreviewProxy,
  PREVIEW_API_ORIGIN_ENV,
  PREVIEW_PROXIED_PREFIXES,
} from '../../vite/frontendPreviewProxy';

/**
 * The frontend preview's Vite proxy. Lives under `src/__tests__/` because that
 * is the only tree Vitest's `include` covers, but the module under test is
 * build config (`crates/lucidos-app/vite/`), deliberately out of the app bundle.
 */
describe('frontendPreviewProxy', () => {
  it('is absent without the engine-supplied origin, so `npm run dev` is untouched', () => {
    // The one regression this guards: `server` is also the standalone dev
    // server's config, and a proxy pointing at a dead origin would break it.
    expect(frontendPreviewProxy(undefined)).toBeUndefined();
    expect(frontendPreviewProxy('')).toBeUndefined();
    expect(frontendPreviewProxy('   ')).toBeUndefined();
  });

  it('forwards exactly the engine-owned prefixes', () => {
    const proxy = frontendPreviewProxy('https://127.0.0.1:5173');
    expect(Object.keys(proxy ?? {}).sort()).toEqual(['/api', '/app', '/data']);
  });

  it('accepts the self-signed dev cert and rewrites the Host', () => {
    const proxy = frontendPreviewProxy('https://127.0.0.1:5173');
    for (const prefix of PREVIEW_PROXIED_PREFIXES) {
      expect(proxy?.[prefix]).toEqual({
        target: 'https://127.0.0.1:5173',
        changeOrigin: true,
        // The engine serves its own self-signed cert in dev; refusing it here
        // would make every proxied request fail on a working setup.
        secure: false,
      });
    }
  });

  it('carries the same env-var name the engine sets', () => {
    // Mirrored in `engine::frontend_preview::PREVIEW_API_ORIGIN_ENV`. A drift
    // between the two is silent: the proxy just never appears.
    expect(PREVIEW_API_ORIGIN_ENV).toBe('LUCIDOS_FRONTEND_PREVIEW_API_ORIGIN');
  });
});
