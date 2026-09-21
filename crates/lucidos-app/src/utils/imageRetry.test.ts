// @vitest-environment jsdom
// `retrySrc` resolves a src against the page origin, so it needs a real one.
import { describe, it, expect } from 'vitest';
import { isRetryableImageSrc, retrySrc, retryDelayMs, MAX_ATTEMPTS } from './imageRetry';

// Root cause: a bare <img> that fails a transient load never re-fetches. The
// element stays broken while tapping to open it creates a fresh one that
// succeeds. Both callers self-heal by re-requesting with a cache-busted URL.

describe('isRetryableImageSrc', () => {
  it('retries server-fetched URLs', () => {
    expect(isRetryableImageSrc('/dev/api/v1/blobs/abc/preview')).toBe(true);
    expect(isRetryableImageSrc('https://host/api/v1/blobs/abc/preview')).toBe(true);
    expect(isRetryableImageSrc('/dev/data/artifacts/shot.png')).toBe(true);
  });

  it('does not retry in-memory object/data URLs (they cannot recover over the network)', () => {
    expect(isRetryableImageSrc('blob:https://host/uuid')).toBe(false);
    expect(isRetryableImageSrc('data:image/png;base64,AAAA')).toBe(false);
  });
});

describe('retrySrc', () => {
  it('attempt 0 is the clean happy-path URL (clean SW cache key)', () => {
    const url = '/dev/api/v1/blobs/abc/preview';
    expect(retrySrc(url, 0)).toBe(url);
  });

  it('cache-busts retries so the browser/SW actually re-fetch', () => {
    expect(retrySrc('/api/v1/blobs/abc/preview', 1)).toBe('/api/v1/blobs/abc/preview?retry=1');
    expect(retrySrc('/api/v1/blobs/abc/preview', 2)).toBe('/api/v1/blobs/abc/preview?retry=2');
  });

  it('uses & when the URL already has a query string', () => {
    expect(retrySrc('/api/v1/blobs/abc/preview?x=1', 1)).toBe('/api/v1/blobs/abc/preview?x=1&retry=1');
  });

  it('never mutates a blob:/data: URL even on a retry', () => {
    expect(retrySrc('blob:https://host/uuid', 3)).toBe('blob:https://host/uuid');
  });

  it('busts an absolute URL this origin serves', () => {
    const url = `${location.origin}/dev/data/artifacts/shot.png`;
    expect(retrySrc(url, 1)).toBe(`${url}?retry=1`);
  });

  // Markdown can embed any third-party image, and a signature covers the exact
  // query string. An unsigned retry= on the end of one 403s every attempt.
  it('re-requests a cross-origin URL unchanged, signature intact', () => {
    const signed = 'https://cdn.example.test/a.png?X-Amz-Signature=abc';
    expect(retrySrc(signed, 1)).toBe(signed);
  });
});

describe('retryDelayMs', () => {
  it('backs off exponentially and caps', () => {
    expect(retryDelayMs(0)).toBe(800);
    expect(retryDelayMs(1)).toBe(1600);
    expect(retryDelayMs(2)).toBe(3200);
    // Capped well before MAX_ATTEMPTS exhausts.
    expect(retryDelayMs(MAX_ATTEMPTS)).toBe(15000);
  });
});
