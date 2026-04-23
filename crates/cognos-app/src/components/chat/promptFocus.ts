import { isElementVisible } from './scrollState';

/** Focus an element only if it isn't already the active element, avoiding flicker.
 *  Uses preventScroll to avoid iOS Safari auto-scrolling overflow:hidden
 *  containers (e.g. the mobile swipe container) when the target is offscreen. */
export function focusIfNeeded(el: HTMLElement | null | undefined): void {
  if (el && document.activeElement !== el) {
    el.focus({ preventScroll: true });
  }
}

/**
 * Find the visible prompt textarea. Both desktop (SplitLayout) and mobile
 * (MobileSwipeContainer) render PromptInput, creating two elements with
 * data-role="prompt-input". On mobile the desktop one is inside
 * `.content-row { display: none }` and has a 0x0 rect. We query all and
 * pick the one with non-zero dimensions.
 */
function getVisiblePromptInput(): HTMLElement | null {
  const els = document.querySelectorAll<HTMLElement>('[data-role="prompt-input"]');
  for (const el of els) {
    if (isElementVisible(el)) return el;
  }
  // Fallback: if none have dimensions (e.g. during initial render), return
  // the last one — on mobile it's the mobile-layout textarea.
  return els.length > 0 ? els[els.length - 1] : null;
}

/**
 * Focus the prompt textarea. Must be called synchronously within a user
 * gesture (touch/click) — iOS Safari only opens the keyboard when focus()
 * is in the gesture's call stack. Deferred paths (useEffect, rAF) get the
 * focus ring but no keyboard.
 *
 * Uses preventScroll because the target may be in an offscreen swipe pane.
 * iOS Safari auto-scrolls overflow:hidden containers to reveal focused
 * elements, which permanently offsets the swipe track (the "half panel" bug).
 * preventScroll still opens the keyboard within the gesture window.
 */
export function focusPromptNow(): void {
  const el = getVisiblePromptInput();
  if (el) el.focus({ preventScroll: true });
}

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
  let handledByTouch = false;
  return {
    onTouchEnd: (e: TouchEvent) => {
      handledByTouch = true;
      e.preventDefault();
      focusFn();
      action();
    },
    onClick: () => {
      if (handledByTouch) {
        handledByTouch = false;
        return;
      }
      focusFn();
      action();
    },
  };
}
