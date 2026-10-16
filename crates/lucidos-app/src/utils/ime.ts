/** The legacy `keyCode` a browser reports for a key the IME swallowed. */
const IME_HANDLED_KEYCODE = 229;

/** Whether this keydown belongs to an IME composition, and so is not a command.
 *
 *  A Japanese, Chinese or Korean user presses Enter to COMMIT the candidate the
 *  IME is showing. The browser dispatches that keydown before `compositionend`,
 *  so an ungated Enter branch acts on the half-converted text. The composer
 *  would send it, and the prompt dialog would submit it.
 *
 *  `isComposing` is the standard reading, and the one `shouldTypeToFocusPrompt`
 *  takes in `hooks/useKeyboardShortcuts.ts`. The `keyCode` arm covers browsers
 *  that report only the sentinel. */
export function isImeComposingKey(e: Pick<KeyboardEvent, 'isComposing' | 'keyCode'>): boolean {
  return e.isComposing || e.keyCode === IME_HANDLED_KEYCODE;
}
