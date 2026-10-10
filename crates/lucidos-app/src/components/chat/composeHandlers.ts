import { touchActivated } from '../../utils/tapGesture';
import { focusPromptNow } from './promptFocus';

/**
 * Return a touchend+click handler pair for buttons that should focus an
 * input on iOS Safari PWA. iOS ties its keyboard-opening gesture window
 * to touch events, not click. The synthetic `click` fires ~300ms after
 * the touch — often outside the window, producing a focus ring but no
 * keyboard. Using `touchend` guarantees we're inside the gesture window.
 *
 * Focus BEFORE action: the action (e.g. unfocusThread) triggers a Preact
 * signal re-render. If the re-render runs between the touch event and
 * focus(), iOS considers the gesture expired — focus ring but no keyboard.
 * Focusing first opens the keyboard within the gesture; the re-render
 * preserves focus because the textarea DOM node stays the same.
 *
 * @param action  Callback to run after focusing
 * @param focusFn Focus function — defaults to focusPromptNow()
 *
 * Usage: <button {...composeHandlers(() => unfocusThread())} />
 */
export function composeHandlers(action: () => void, focusFn: () => void = focusPromptNow) {
  return touchActivated(() => {
    focusFn();
    action();
  });
}
