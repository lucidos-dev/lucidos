import { describe, it, expect } from 'vitest';
import { isRetryableBlobSrc, retrySrc, retryDelayMs, MAX_ATTEMPTS } from '../BlobImage';

// Root cause: a bare <img> that fails a transient load (engine restart drops
// every socket; iOS PWA wake-from-suspension) never re-fetches, so the inline
// thumbnail stays a broken box — while tapping to open creates a fresh element
// that succeeds. BlobImage self-heals by re-requesting with a cache-busted URL.

describe('isRetryableBlobSrc', () => {
  it('retries server-fetched URLs', () => {
    expect(isRetryableBlobSrc('/dev/api/v1/blobs/abc/preview')).toBe(true);
    expect(isRetryableBlobSrc('https://host/api/v1/blobs/abc/preview')).toBe(true);
  });

  it('does not retry in-memory object/data URLs (they cannot recover over the network)', () => {
    expect(isRetryableBlobSrc('blob:https://host/uuid')).toBe(false);
    expect(isRetryableBlobSrc('data:image/png;base64,AAAA')).toBe(false);
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
