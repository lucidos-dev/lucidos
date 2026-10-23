// Refuse a text insertion that is nothing but key codes.
//
// WHAT THE USER SEES: an arrow key types a square when the caret has nowhere
// to go, such as the right arrow at the end of the prompt. Desktop app only.
//
// WHY: WebKit hands back a key press its editing command declined. Tauri's
// window view then runs the event through macOS text input a second time, and
// that inserts the event's characters into the focused field. A macOS arrow
// key carries a control code, 0x1C to 0x1F, which the field draws as a square.
// `docs/temporary-measures.md` holds the full mechanism and the removal
// condition, under "macOS key codes inserted as text at a caret boundary".
//
// NOT GATED ON TAURI. A key code is never text on any platform, so refusing it
// needs no platform branch. The rule is `isKeyCodeTextInsertion`, shared with
// the SDK, whose own guard covers app frames this listener cannot reach.
import { isKeyCodeTextInsertion } from '@lucidos/text-entry';

let installed = false;

/** Install the global key-code text guard. Idempotent. Capture phase, so the
 *  cancel lands before any field's own `beforeinput` handler. */
export function installNoKeyCodeText(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    'beforeinput',
    (e) => {
      if (isKeyCodeTextInsertion(e.inputType, e.data)) e.preventDefault();
    },
    { capture: true },
  );
}
