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

/** The block-level image `renderMarkdown` produces from `![alt](src)`, inside
 *  the scroll wrapper that sizes it. One selector reaches every markdown
 *  surface: a chat turn, a rendered `.md` preview, a notification body. It
 *  mirrors the rule that caps the image in shared-components.css, so the
 *  `zoom-in` cursor and the click land on the same set. */
const INLINE_MARKDOWN_IMAGE = '.markdown-content .image-scroll-wrapper > img';

/** The inline markdown image a click landed on, or null.
 *
 *  A linked image is left alone. The author gave that click a destination, and
 *  opening the popup would steal it. */
export function inlineMarkdownImage(target: EventTarget | null): HTMLImageElement | null {
  if (!(target instanceof Element)) return null;
  const img = target.closest<HTMLImageElement>(INLINE_MARKDOWN_IMAGE);
  return img && !img.closest('a') ? img : null;
}

/** Open the popup with prev/next nav across every sibling thumbnail in the
 *  nearest image group around `clicked`. That group is a sent-message thread
 *  (`.thread-content`), the unsent prompt strip (`.image-preview-strip`), or,
 *  for a surface outside the thread pane, the markdown block itself.
 *  Degrades to single-image when no siblings can be collected. */
export function openImagePopupFromGroup(src: string, clicked: Element | EventTarget | null): void {
  const el = (clicked instanceof Element) ? clicked : null;
  // The thread and the strip are tried first, so a transcript stays ONE group
  // rather than splitting per turn at the `.markdown-content` inside it.
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
