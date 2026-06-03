import { signal } from '@preact/signals';

// --- Image popup ---
export interface ImagePopupState {
  images: string[];
  index: number;
}

export const popupImage = signal<ImagePopupState | null>(null);

export function openImagePopup(src: string): void {
  popupImage.value = { images: [src], index: 0 };
}

/** Open the popup with prev/next nav across every sibling thumbnail in the
 *  nearest image group around `clicked` — either a sent-message thread
 *  (`.thread-content`) or the unsent prompt strip (`.image-preview-strip`).
 *  Degrades to single-image when no siblings can be collected. */
export function openImagePopupFromGroup(src: string, clicked: Element | EventTarget | null): void {
  const container = (clicked instanceof Element) ? clicked.closest('.thread-content, .image-preview-strip') : null;
  if (!container) { openImagePopup(src); return; }
  const els = container.querySelectorAll<HTMLImageElement>('.image-thumbnail, .user-image-thumb, .image-preview-thumb');
  const seen = new Set<string>();
  const images: string[] = [];
  els.forEach(el => {
    const url = el.dataset.fullSrc || el.src;
    if (url && !seen.has(url)) { seen.add(url); images.push(url); }
  });
  const index = images.indexOf(src);
  if (index === -1) { openImagePopup(src); return; }
  popupImage.value = { images, index };
}
