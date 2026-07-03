import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { JSX } from 'preact';

/** Stop retrying after this many failed loads — beyond it the element stays
 *  broken (same as a bare <img>), so a genuinely-missing blob doesn't loop. */
export const MAX_ATTEMPTS = 6;
const BASE_DELAY_MS = 800;
const MAX_DELAY_MS = 15000;

/** `blob:` / `data:` URLs are in-memory bytes — a failed load can't recover over
 *  the network, so retrying is pointless. Only server-fetched URLs are retried. */
export function isRetryableBlobSrc(src: string): boolean {
  return !src.startsWith('blob:') && !src.startsWith('data:');
}

/** Cache-bust a retry so the browser and service worker actually re-fetch
 *  instead of replaying the cached failure. Attempt 0 is the clean happy-path
 *  URL (and a clean SW cache key); the `retry=N` param only appears on retries.
 *  Blob bytes are immutable for the hash, so the engine ignores the param. */
export function retrySrc(src: string, attempt: number): string {
  if (attempt === 0 || !isRetryableBlobSrc(src)) return src;
  return `${src}${src.includes('?') ? '&' : '?'}retry=${attempt}`;
}

/** Exponential backoff capped at MAX_DELAY_MS — keeps re-requesting through a
 *  multi-second engine restart without hammering it. */
export function retryDelayMs(attempt: number): number {
  return Math.min(BASE_DELAY_MS * 2 ** attempt, MAX_DELAY_MS);
}

type BlobImageProps = Omit<JSX.IntrinsicElements['img'], 'src'> & {
  src: string;
};

/** An `<img>` for content-addressed blob previews that self-heals when a load
 *  fails transiently. A bare `<img>` never re-fetches after an error: a load
 *  that lands during an engine restart (Apply & Restart drops every socket) or
 *  an iOS PWA wake-from-suspension leaves the thumbnail PERMANENTLY broken until
 *  the element is recreated — which is exactly why tapping to open it (a fresh
 *  `<img>` in the popup) works while the inline preview stays a broken box.
 *
 *  On error we re-request with an exponential backoff, cache-busting the URL with
 *  a `retry=N` query param so the browser and service worker actually re-fetch
 *  rather than replay the cached failure. The blob bytes are immutable for the
 *  hash, so the engine ignores the extra param and the param only appears on
 *  retries — the happy-path first load stays a clean URL (and a clean SW cache
 *  key). `blob:` URLs are in-memory session bytes that can't recover over the
 *  network, so they're never retried. */
export function BlobImage({ src, onError, onLoad, ...rest }: BlobImageProps) {
  const attempt = useSignal(0);
  const timer = useRef<number | null>(null);
  const effectiveSrc = retrySrc(src, attempt.value);

  // A new src is a fresh image — reset the retry count so it can't inherit a
  // leftover `?retry=N` from the previous one (keeps the happy-path URL clean).
  // The cleanup also cancels a pending retry when src changes or the element
  // unmounts, so a backoff timer never fires against a gone instance.
  useEffect(() => {
    attempt.value = 0;
    return () => {
      if (timer.current !== null) {
        clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [src]);

  const handleError: JSX.GenericEventHandler<HTMLImageElement> = (e) => {
    if (typeof onError === 'function') onError(e);
    if (!isRetryableBlobSrc(src) || attempt.value >= MAX_ATTEMPTS || timer.current !== null) return;
    timer.current = window.setTimeout(() => {
      timer.current = null;
      attempt.value += 1;
    }, retryDelayMs(attempt.value));
  };

  const handleLoad: JSX.GenericEventHandler<HTMLImageElement> = (e) => {
    // A late-arriving success cancels any pending retry.
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    if (typeof onLoad === 'function') onLoad(e);
  };

  return <img src={effectiveSrc} onError={handleError} onLoad={handleLoad} {...rest} />;
}
