
import { showToast } from '../store/store';
import { clipboardAbilities } from './platform';

const NO_CLIPBOARD = 'No clipboard access on this address.';

export function clipboardOrReport(): Clipboard | null {
  const clipboard = navigator.clipboard;
  if (clipboard) return clipboard;
  showToast(`${NO_CLIPBOARD} Select the text and copy it by hand.`, 'error');
  return null;
}

export function copyToClipboard(text: string, copied = 'Copied to clipboard'): void {
  const clipboard = clipboardOrReport();
  if (!clipboard) return;
  clipboard.writeText(text).then(
    () => showToast(copied, 'success'),
    () => showToast('Failed to copy', 'error'),
  );
}

const PNG = 'image/png';

export async function fetchImageAsPng(src: string): Promise<Blob> {
  const response = await fetch(src);
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const blob = await response.blob();
  return blob.type === PNG ? blob : redrawAsPng(blob);
}

async function redrawAsPng(blob: Blob): Promise<Blob> {
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    if (!img.naturalWidth || !img.naturalHeight) throw new Error('the image has no size');
    const canvas = document.createElement('canvas');
    canvas.width = img.naturalWidth;
    canvas.height = img.naturalHeight;
    canvas.getContext('2d')?.drawImage(img, 0, 0);
    return await new Promise<Blob>((resolve, reject) => {
      canvas.toBlob((png) => (png ? resolve(png) : reject(new Error('the canvas gave no PNG'))), PNG);
    });
  } finally {
    URL.revokeObjectURL(url);
  }
}

export function copyImageToClipboard(src: string): void {
  if (!clipboardAbilities().copyImage) {
    showToast(NO_CLIPBOARD, 'error');
    return;
  }
  const png = fetchImageAsPng(src);
  png.catch(() => {});
  const failed = (err: unknown) =>
    showToast(`Failed to copy image: ${err instanceof Error ? err.message : String(err)}`, 'error');
  let item: ClipboardItem;
  try {
    item = new ClipboardItem({ [PNG]: png });
  } catch (err) {
    failed(err);
    return;
  }
  navigator.clipboard.write([item]).then(
    () => showToast('Image copied', 'success'),
    (err) => png.then(() => failed(err), failed),
  );
}
