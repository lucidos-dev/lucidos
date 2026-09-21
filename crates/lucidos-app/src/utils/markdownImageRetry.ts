import { MAX_ATTEMPTS, isRetryableImageSrc, retryDelayMs, retrySrc } from './imageRetry';

/** Re-request a markdown image whose load failed, so a transient failure heals
 *  itself instead of leaving the picture half-drawn.
 *
 *  `renderMarkdown` emits raw `<img>` markup that every surface mounts through
 *  `dangerouslySetInnerHTML`, so those images can't be `<BlobImage>` elements
 *  and get none of its retry. The failure that shows is a truncated response.
 *  The browser reads the header, lays the element out at the right size, paints
 *  the rows it received, and fires `error` on the rest. The reported symptom
 *  was a screenshot whose top inch was correct above a tall empty box, whole
 *  again the moment the thread was reopened.
 *
 *  One document-level listener covers every markdown surface: a chat turn, a
 *  rendered `.md` preview, a notification body. It sits in the CAPTURE phase
 *  because a resource `error` does not bubble. */

interface Retry {
  /** The `src` as the markup asked for it. A re-request overwrites the
   *  attribute with a cache-busted URL, so the next one reads its clean base
   *  here. Otherwise it would stack a second `retry=` param onto the first. */
  base: string;
  /** Re-requests spent, over the element's whole life. Monotonic on purpose. It
   *  bounds the budget, and it keeps every busted URL distinct, so no request
   *  repeats one the browser already holds a broken entry for. */
  attempt: number;
  timer: number | null;
}

const retries = new WeakMap<HTMLImageElement, Retry>();

function markdownImage(target: EventTarget | null): HTMLImageElement | null {
  if (!(target instanceof HTMLImageElement)) return null;
  return target.closest('.markdown-content') ? target : null;
}

/** This image's retry record, created on its first failure. Null when the src
 *  can't recover over the network, which is the whole reason to skip it. */
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
    // A re-render replaces the markup wholesale, so an element that left the
    // document is nothing to re-request.
    if (!img.isConnected) return;
    // Counted here, not at schedule time, so a retry a success cancelled costs
    // the budget nothing.
    retry.attempt += 1;
    img.src = retrySrc(retry.base, retry.attempt);
  }, retryDelayMs(retry.attempt));
}

/** A late-arriving success cancels the pending retry, which would otherwise
 *  re-request an image that is already on screen. The record itself stays, so a
 *  later failure still busts the clean base. Keyed on the WeakMap alone: every
 *  image in the app fires `load`, and only one that already failed is worth a
 *  DOM walk. */
function onImageLoad(event: Event): void {
  const img = event.target;
  if (!(img instanceof HTMLImageElement)) return;
  const retry = retries.get(img);
  if (!retry || retry.timer === null) return;
  clearTimeout(retry.timer);
  retry.timer = null;
}

export function installMarkdownImageRetry(): () => void {
  document.addEventListener('error', onImageError, { capture: true });
  document.addEventListener('load', onImageLoad, { capture: true });
  return () => {
    document.removeEventListener('error', onImageError, { capture: true } as EventListenerOptions);
    document.removeEventListener('load', onImageLoad, { capture: true } as EventListenerOptions);
  };
}
