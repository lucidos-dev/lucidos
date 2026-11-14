/**
 * Copy text or an image, and tell the user either way.
 *
 * A copy that fails is otherwise silent: the user taps Copy, nothing lands on
 * the clipboard, and nothing on screen says so. Reporting it is the whole
 * point, and it is the same report at every call site.
 *
 * There are TWO ways to fail, and the first is the one that bites. A non-secure
 * context has no `navigator.clipboard` at all. The unguarded call then throws
 * before a promise exists, so neither arm of a `then` pair runs. Every caller
 * can be read over a plain-HTTP LAN address, which is that context, so the
 * guard belongs here rather than at each button.
 *
 * A surface with somewhere better to put the text should ALSO hide its button,
 * on `clipboardAbilities()` from `utils/platform`. A control that can only
 * fail is worse than no control.
 */

import { showToast } from '../store/store';
import { clipboardAbilities } from './platform';

const NO_CLIPBOARD = 'No clipboard access on this address.';

/** The clipboard, or `null` after telling the user there is none here. For a
 *  caller that reports success its own way (a button swapping to a tick) and so
 *  cannot use {@link copyToClipboard}. It still owes the user the failure. */
export function clipboardOrReport(): Clipboard | null {
  const clipboard = navigator.clipboard;
  if (clipboard) return clipboard;
  showToast(`${NO_CLIPBOARD} Select the text and copy it by hand.`, 'error');
  return null;
}

/** Put `text` on the clipboard. `copied` names what landed there. */
export function copyToClipboard(text: string, copied = 'Copied to clipboard'): void {
  const clipboard = clipboardOrReport();
  if (!clipboard) return;
  clipboard.writeText(text).then(
    () => showToast(copied, 'success'),
    () => showToast('Failed to copy', 'error'),
  );
}

const PNG = 'image/png';

/** The picture at `src` as a PNG blob. The clipboard takes PNG everywhere and
 *  little else, so another format is redrawn through a canvas. A PNG goes
 *  through byte for byte. */
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

/** Put the picture at `src` on the clipboard. Call it from the tap itself.
 *
 *  The `ClipboardItem` is built synchronously, with a promise for its data.
 *  iOS Safari accepts a write only inside the tap's own task, and a fetch
 *  cannot finish there. */
export function copyImageToClipboard(src: string): void {
  if (!clipboardAbilities().copyImage) {
    showToast(NO_CLIPBOARD, 'error');
    return;
  }
  const png = fetchImageAsPng(src);
  // Marks the rejection handled. Its reason is read below, and a browser that
  // never reads the data would otherwise log it as unhandled.
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
    // A failed fetch surfaces from the write as a generic error, so name the
    // fetch's own reason when it has one.
    (err) => png.then(() => failed(err), failed),
  );
}
