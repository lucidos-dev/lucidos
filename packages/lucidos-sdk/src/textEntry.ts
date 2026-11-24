
import { parseFlag, PREF_AUTOCORRECT } from './generated/preference-catalog';

export const NON_TEXT_INPUT_TYPES: ReadonlySet<string> = new Set([
  'button', 'submit', 'reset', 'image', 'file',
  'checkbox', 'radio', 'range', 'color', 'hidden',
]);

export function isTextEntryField(el: Element): el is HTMLInputElement | HTMLTextAreaElement {
  if (el.localName === 'textarea') return true;
  if (el.localName === 'input') return !NON_TEXT_INPUT_TYPES.has((el as HTMLInputElement).type);
  return false;
}

export const AUTOCORRECT_STORAGE_KEY = 'lucidos-autocorrect';

export function defaultAutocorrect(): boolean {
  return PREF_AUTOCORRECT.fallback === 'true';
}

export function resolveAutocorrect(raw: string | null | undefined): boolean {
  return parseFlag(raw ?? '') ?? defaultAutocorrect();
}

function isKeyCode(code: number): boolean {
  if (code === 0x09 || code === 0x0a || code === 0x0d) return false;
  if (code < 0x20 || code === 0x7f) return true;
  return code >= 0xf700 && code <= 0xf747;
}

/** Whether a `beforeinput` inserts nothing but key codes, so it must be
 *  cancelled.
 *
 *  The desktop app's web view types one when an arrow key has nowhere to move
 *  the caret, and the field shows it as a square. `docs/temporary-measures.md`
 *  holds the mechanism and the removal condition.
 *
 *  Every character has to be one. A paste carries its content on
 *  `dataTransfer` and leaves `data` null, so pasted text is never refused. */
export function isKeyCodeTextInsertion(inputType: string, data: string | null): boolean {
  if (!inputType.startsWith('insert') || !data) return false;
  for (const char of data) {
    if (!isKeyCode(char.codePointAt(0) ?? 0)) return false;
  }
  return true;
}
