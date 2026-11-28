import { signal } from '@preact/signals';

export interface ImagePopupState {
  images: string[];
  index: number;
}

export const popupImage = signal<ImagePopupState | null>(null);

export function openImagePopup(src: string): void {
  popupImage.value = { images: [src], index: 0 };
}

export function openImageGallery(images: string[], index: number): void {
  popupImage.value = { images, index };
}

const INLINE_MARKDOWN_IMAGE = '.markdown-content .image-scroll-wrapper > img, .question-body .image-scroll-wrapper > img';

export function inlineMarkdownImage(target: EventTarget | null): HTMLImageElement | null {
  if (!(target instanceof Element)) return null;
  const img = target.closest<HTMLImageElement>(INLINE_MARKDOWN_IMAGE);
  return img && !img.closest('a') ? img : null;
}

export function openImagePopupFromGroup(src: string, clicked: Element | EventTarget | null): void {
  const el = (clicked instanceof Element) ? clicked : null;
  const container = el?.closest('.thread-content, .image-preview-strip')
    ?? el?.closest('.markdown-content');
  if (!container) { openImagePopup(src); return; }
  const els = container.querySelectorAll<HTMLImageElement>(
    `.image-thumbnail, .user-image-thumb, .image-preview-thumb, ${INLINE_MARKDOWN_IMAGE}`,
  );
  const seen = new Set<string>();
  const images: string[] = [];
  els.forEach(img => {
    const url = img.dataset.fullSrc || img.src;
    if (url && !seen.has(url)) { seen.add(url); images.push(url); }
  });
  const index = images.indexOf(src);
  if (index === -1) { openImagePopup(src); return; }
  popupImage.value = { images, index };
}
