/**
 * Copy text, and tell the user either way.
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
 * on `clipboardAbilities().copy` from `utils/platform`. A control that can only
 * fail is worse than no control.
 */

import { showToast } from '../store/store';

/** The clipboard, or `null` after telling the user there is none here. For a
 *  caller that reports success its own way (a button swapping to a tick) and so
 *  cannot use {@link copyToClipboard}. It still owes the user the failure. */
export function clipboardOrReport(): Clipboard | null {
  const clipboard = navigator.clipboard;
  if (clipboard) return clipboard;
  showToast('No clipboard access on this address. Select the text and copy it by hand.', 'error');
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
