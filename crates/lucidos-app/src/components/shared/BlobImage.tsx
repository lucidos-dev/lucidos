import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import type { JSX } from 'preact';
import { MAX_ATTEMPTS, isRetryableImageSrc, retryDelayMs, retrySrc } from '../../utils/imageRetry';

type BlobImageProps = Omit<JSX.IntrinsicElements['img'], 'src'> & {
  src: string;
};

export function BlobImage({ src, onError, onLoad, ...rest }: BlobImageProps) {
  const attempt = useSignal(0);
  const timer = useRef<number | null>(null);
  const effectiveSrc = retrySrc(src, attempt.value);

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
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    if (typeof onLoad === 'function') onLoad(e);
  };

  return <img src={effectiveSrc} onError={handleError} onLoad={handleLoad} {...rest} />;
}
