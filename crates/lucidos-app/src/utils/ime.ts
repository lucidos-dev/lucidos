const IME_HANDLED_KEYCODE = 229;

export function isImeComposingKey(e: Pick<KeyboardEvent, 'isComposing' | 'keyCode'>): boolean {
  return e.isComposing || e.keyCode === IME_HANDLED_KEYCODE;
}
