/**
 * Refuse a key code typed as text into an app frame's own fields.
 *
 * The host refuses it in its own document (`utils/noKeyCodeText.ts`), and that
 * listener cannot reach into this one. Without this guard, an arrow key at the
 * end of an app's text field types a square in the desktop app. The rule is
 * `isKeyCodeTextInsertion` in `textEntry.ts`, shared with the host.
 */
import { isKeyCodeTextInsertion } from './textEntry';

let installed = false;

/** Install the guard. Idempotent. Capture phase, so the cancel lands before any
 *  field's own `beforeinput` handler. */
export function installNoKeyCodeText(): void {
  if (installed) return;
  installed = true;
  document.addEventListener('beforeinput', (e) => {
    if (isKeyCodeTextInsertion(e.inputType, e.data)) e.preventDefault();
  }, { capture: true });
}
