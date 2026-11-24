
export const IMAGE_UNAVAILABLE_CLASS = 'markdown-image-unavailable';
const FAILED_ATTR = 'data-load-failed';

const RENDERED_IMAGE_WRAPPER = 'image-scroll-wrapper';

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

export function markImageUnavailable(img: HTMLImageElement): void {
  img.after(buildNotice(img));
  img.setAttribute(FAILED_ATTR, '');
  img.removeAttribute('src');
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
