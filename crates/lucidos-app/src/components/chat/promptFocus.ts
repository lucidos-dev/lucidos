import { isElementVisible } from './scrollState';

/** Focus an element only if it isn't already the active element, avoiding flicker.
 *  Uses preventScroll to avoid iOS Safari auto-scrolling overflow:hidden
 *  containers (e.g. the mobile swipe container) when the target is offscreen. */
export function focusIfNeeded(el: HTMLElement | null | undefined): void {
  if (el && document.activeElement !== el) {
    el.focus({ preventScroll: true });
  }
}

/** The currently focused prompt textarea, or null if focus is elsewhere.
 *  Reads document.activeElement directly rather than locating the visible
 *  prompt-input and comparing: focus is exactly the question being asked, so the
 *  active element answers it without depending on layout or measurement. */
function activePromptInput(): HTMLElement | null {
  const el = document.activeElement as HTMLElement | null;
  return el?.dataset?.role === 'prompt-input' ? el : null;
}

/** True when the focused element is a prompt-input textarea bound to threadId.
 *  Gate any SSE / API refresh that would overwrite compose state on this —
 *  otherwise in-flight keystrokes get blanked and the cursor jumps to end. */
export function isComposeFocusedHere(threadId: string): boolean {
  return activePromptInput()?.dataset.threadId === threadId;
}

/**
 * Find the visible prompt textarea. `App` mounts only the active layout's pane
 * tree (SplitLayout on desktop, MobileSwipeContainer on mobile), so there is
 * normally a single `data-role="prompt-input"` element. It can still be laid
 * out at zero size (a collapsed thread pane, or a pane not measured yet), which
 * is not the same as being the one to focus, so query them all, prefer one with
 * real dimensions, and fall back to the last match.
 */
export function getVisiblePromptInput(): HTMLElement | null {
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

/** Blur the prompt textarea iff it is the active element. No-op otherwise.
 *  Action buttons exit the compose state, so the mobile keyboard the user
 *  raised while composing should drop with the tap. */
export function blurPromptInputIfFocused(): void {
  activePromptInput()?.blur();
}

/** Install a document-level listener that blurs the prompt textarea on any
 *  `.action-btn` tap. Idempotent — safe to call from module init. Action
 *  buttons (Send / Apply / Archive / Cancel / Discard / Diff / Continue /
 *  Revert) never want the mobile keyboard to remain up or to surface via
 *  implicit focus retention.
 *
 *  Listen on `click` (bubble phase) — NOT `pointerdown`. Blurring on
 *  pointerdown triggers iOS Safari's animated keyboard dismissal mid-tap;
 *  the visual viewport shifts and the button moves out from under the
 *  finger before the synthesized click can dispatch, dropping the click
 *  entirely. The user then has to tap a second time to actually trigger
 *  the action. Listening on click lets the button's own handler dispatch
 *  first — the keyboard dismisses cleanly afterward. */
let actionBtnBlurInstalled = false;
export function installActionBtnBlurListener(): void {
  if (actionBtnBlurInstalled) return;
  actionBtnBlurInstalled = true;
  document.addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null;
    if (!target?.closest('.action-btn')) return;
    blurPromptInputIfFocused();
  });
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
