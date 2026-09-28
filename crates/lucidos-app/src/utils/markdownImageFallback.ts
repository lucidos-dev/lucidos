/** Swap a markdown image that failed to load for a notice saying so.
 *
 *  A broken `<img>` draws the browser's own glyph: an empty frame, a `?` on
 *  WebKit, no words. The notice names the alt text and the source, so the
 *  reader learns what was meant and where it pointed. The usual cause is an
 *  agent pasting a path on its own disk, which no client can load.
 *
 *  It coexists with `markdownImageRetry.ts`. The notice goes up on the first
 *  failure, and a retry that later loads takes it down again.
 *
 *  Delegated from `document` in the CAPTURE phase, like the retry, because a
 *  resource `error` does not bubble. */

export const IMAGE_UNAVAILABLE_CLASS = 'markdown-image-unavailable';
const FAILED_ATTR = 'data-load-failed';

/** `renderMarkdown` wraps every image it writes in this, on every surface. A
 *  JSX `<img>` never has it, so the notice is only ever inserted into raw
 *  markup, never among the siblings Preact reconciles. */
const RENDERED_IMAGE_WRAPPER = 'image-scroll-wrapper';

/** In-memory bytes. Their source is the payload, not a place worth naming. */
const IN_MEMORY_SRC = /^(data|blob):/i;

const notices = new WeakMap<HTMLImageElement, HTMLElement>();

function buildNotice(img: HTMLImageElement): HTMLElement {
  const doc = img.ownerDocument;
  const notice = doc.createElement('span');
  notice.className = IMAGE_UNAVAILABLE_CLASS;
  const alt = img.getAttribute('alt')?.trim();
  notice.append(alt ? `Image not available: ${alt}` : 'Image not available');
  const src = img.getAttribute('src') ?? '';
  if (src && !IN_MEMORY_SRC.test(src)) {
    const source = doc.createElement('code');
    source.textContent = src;
    notice.append(' ', source);
  }
  return notice;
}

function onImageError(event: Event): void {
  const img = event.target;
  if (!(img instanceof HTMLImageElement)) return;
  if (!img.parentElement?.classList.contains(RENDERED_IMAGE_WRAPPER)) return;
  if (notices.has(img)) return;
  const notice = buildNotice(img);
  notices.set(img, notice);
  img.setAttribute(FAILED_ATTR, '');
  img.after(notice);
}

function onImageLoad(event: Event): void {
  const img = event.target;
  if (!(img instanceof HTMLImageElement)) return;
  const notice = notices.get(img);
  if (!notice) return;
  notices.delete(img);
  notice.remove();
  img.removeAttribute(FAILED_ATTR);
}

export function installMarkdownImageFallback(): () => void {
  document.addEventListener('error', onImageError, { capture: true });
  document.addEventListener('load', onImageLoad, { capture: true });
  return () => {
    document.removeEventListener('error', onImageError, { capture: true } as EventListenerOptions);
    document.removeEventListener('load', onImageLoad, { capture: true } as EventListenerOptions);
  };
}
