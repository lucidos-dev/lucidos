import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { JSX } from 'preact';
import { MAX_ATTEMPTS, isRetryableImageSrc, retryDelayMs, retrySrc } from '../../utils/imageRetry';

type BlobImageProps = Omit<JSX.IntrinsicElements['img'], 'src'> & {
  src: string;
};

/** An `<img>` for content-addressed blob previews that self-heals when a load
 *  fails transiently. Two failures reach it: an engine restart, which drops
 *  every socket, and an iOS PWA wake from suspension. Either leaves a bare
 *  `<img>` PERMANENTLY broken until the element is recreated. That is why
 *  tapping to open it works: the popup builds a fresh `<img>`.
 *
 *  The retry policy is `utils/imageRetry.ts`, shared with the delegated handler
 *  that covers the raw `<img>` markdown renders. */
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
    if (!isRetryableImageSrc(src) || attempt.value >= MAX_ATTEMPTS || timer.current !== null) return;
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
