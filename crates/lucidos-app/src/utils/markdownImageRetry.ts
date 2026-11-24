import { MAX_ATTEMPTS, isRetryableImageSrc, retryDelayMs, retrySrc } from './imageRetry';


interface Retry {
  base: string;
  attempt: number;
  timer: number | null;
}

const retries = new WeakMap<HTMLImageElement, Retry>();

function markdownImage(target: EventTarget | null): HTMLImageElement | null {
  if (!(target instanceof HTMLImageElement)) return null;
  return target.closest('.markdown-content') ? target : null;
}

function retryFor(img: HTMLImageElement): Retry | null {
  const existing = retries.get(img);
  if (existing) return existing;
  const base = img.getAttribute('src');
  if (base === null || !isRetryableImageSrc(base)) return null;
  const fresh: Retry = { base, attempt: 0, timer: null };
  retries.set(img, fresh);
  return fresh;
}

function onImageError(event: Event): void {
  const img = markdownImage(event.target);
  if (!img) return;
  const retry = retryFor(img);
  if (!retry) return;
  if (retry.attempt >= MAX_ATTEMPTS || retry.timer !== null) return;
  retry.timer = window.setTimeout(() => {
    retry.timer = null;
    if (!img.isConnected) return;
    retry.attempt += 1;
    img.src = retrySrc(retry.base, retry.attempt);
  }, retryDelayMs(retry.attempt));
}

function onImageLoad(event: Event): void {
  const img = event.target;
  if (!(img instanceof HTMLImageElement)) return;
  const retry = retries.get(img);
  if (!retry || retry.timer === null) return;
  clearTimeout(retry.timer);
  retry.timer = null;
}

export function isImageRetryPending(img: HTMLImageElement): boolean {
  return (retries.get(img)?.timer ?? null) !== null;
}

export function installMarkdownImageRetry(): () => void {
  document.addEventListener('error', onImageError, { capture: true });
  document.addEventListener('load', onImageLoad, { capture: true });
  return () => {
    document.removeEventListener('error', onImageError, { capture: true } as EventListenerOptions);
    document.removeEventListener('load', onImageLoad, { capture: true } as EventListenerOptions);
  };
}
