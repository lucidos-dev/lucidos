/** The self-healing policy two callers share: the `<BlobImage>` component, and
 *  `markdownImageRetry.ts`, which covers the raw `<img>` markdown renders.
 *
 *  A bare `<img>` never re-fetches after an error, so one bad load leaves the
 *  element broken until something recreates it. That is why tapping an image to
 *  open it works (the popup builds a fresh `<img>`) while the inline copy stays
 *  wrong. */

/** Stop retrying after this many failed loads, so a genuinely-missing image
 *  doesn't loop. Beyond it the element stays broken, same as a bare `<img>`. */
export const MAX_ATTEMPTS = 6;
const BASE_DELAY_MS = 800;
const MAX_DELAY_MS = 15000;

/** `blob:` / `data:` URLs are in-memory bytes, so a failed load can't recover
 *  over the network. Only server-fetched URLs are retried. */
export function isRetryableImageSrc(src: string): boolean {
  return !src.startsWith('blob:') && !src.startsWith('data:');
}

/** True for a URL this page's own origin serves, which is every URL we may add
 *  a query param to. Markdown can embed any third-party image, and a signed one
 *  (an S3 presigned URL, say) carries a signature over its exact query string.
 *  An unsigned `retry=N` on the end of that is a 403 on every attempt. */
function servedByThisOrigin(src: string): boolean {
  if (typeof location === 'undefined') return false;
  try {
    return new URL(src, location.href).origin === location.origin;
  } catch {
    return false;
  }
}

/** Cache-bust a retry so the browser and service worker actually re-fetch
 *  instead of replaying the cached failure. Attempt 0 is the clean happy-path
 *  URL (and a clean SW cache key); the `retry=N` param only appears on retries.
 *  Both our own mounts ignore an unknown query param: blob bytes are immutable
 *  for the hash, and the `/data/` mount keys on the path alone.
 *
 *  A cross-origin URL is re-requested unchanged. Re-assigning `src` still
 *  re-runs the load, because the element's current request is broken. */
export function retrySrc(src: string, attempt: number): string {
  if (attempt === 0 || !isRetryableImageSrc(src) || !servedByThisOrigin(src)) return src;
  return `${src}${src.includes('?') ? '&' : '?'}retry=${attempt}`;
}

/** Exponential backoff capped at MAX_DELAY_MS, which keeps re-requesting
 *  through a multi-second engine restart without hammering it. */
export function retryDelayMs(attempt: number): number {
  return Math.min(BASE_DELAY_MS * 2 ** attempt, MAX_DELAY_MS);
}
