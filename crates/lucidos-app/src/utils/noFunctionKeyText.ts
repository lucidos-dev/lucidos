// Refuse a text insertion that is nothing but macOS function-key characters.
//
// WHAT THE USER SEES: the right arrow types a tofu square when the caret is
// already at the end of the prompt. Arrow keys move the caret normally
// anywhere else in the text. Desktop app only.
//
// WHY: AppKit reserves the block from 0xF700 to 0xF8FF for function keys, so
// the key event for the right arrow carries 0xF703 as its characters. macOS
// maps the key to the `moveRight:` editing command, which WebKit runs. At the
// end of the text that command has nothing to move over, so the keystroke falls
// through to plain text insertion. WebKit's guard on that path rejects only
// control characters below 0x20, and this one is above it.

// THE OTHER HALF of the same failure belongs to `install_app_menu`
// (`crates/lucidos-app/src/lib.rs`). Without a complete app menu, macOS finds no
// command for an arrow key at all, and every press inserts its character. That
// is why movement works here. This guard covers what is left: the press the
// command declines.
//
// NOT GATED ON TAURI, unlike `installNoDrag`. That suppressor removes a real
// browser affordance, so it has to stay off the web. A function-key character
// is never text on any platform, so refusing it needs no platform branch. The
// bound below is what keeps the refusal narrow enough to say that.
//
// `docs/temporary-measures.md` carries the row for this guard, under "macOS
// function-key characters inserted as text at a caret boundary". That row holds
// the removal condition: the webview stops delivering the character.

/** AppKit's function-key constants, as codepoints. The arrows sit at 0xF700,
 *  then F1 to F35, Insert, Delete, Home, End and the page keys, up to Mode
 *  Switch at 0xF747. Nothing in that range is text.
 *
 *  Apple reserves the whole block to 0xF8FF, but assigns only up to 0xF747, and
 *  a key event can carry nothing but an assigned one. Stopping at the
 *  assignments leaves the rest of the private-use block alone. A font glyph
 *  inserted from the Character Viewer still reaches the field.
 *
 *  Written as numbers so the bounds read as the constants they mirror, and so
 *  no unprintable character has to sit in this file. */
const FUNCTION_KEY_FIRST = 0xf700;
const FUNCTION_KEY_LAST = 0xf747;

/** Whether a `beforeinput` is inserting nothing but function-key characters.
 *
 *  Every character has to be one, so an insertion that merely CONTAINS one is
 *  left alone. A paste carries its content on `dataTransfer` and leaves `data`
 *  null, so pasted text never reaches the range test at all. */
export function isFunctionKeyTextInsertion(inputType: string, data: string | null): boolean {
  if (!inputType.startsWith('insert')) return false;
  if (!data) return false;
  for (const char of data) {
    const code = char.codePointAt(0) ?? 0;
    if (code < FUNCTION_KEY_FIRST || code > FUNCTION_KEY_LAST) return false;
  }
  return true;
}

let installed = false;

/** Install the global function-key text guard. Idempotent. Capture phase, so
 *  the cancel lands before any field's own `beforeinput` handler. */
export function installNoFunctionKeyText(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    'beforeinput',
    (e) => {
      if (isFunctionKeyTextInsertion(e.inputType, e.data)) e.preventDefault();
    },
    { capture: true },
  );
}
