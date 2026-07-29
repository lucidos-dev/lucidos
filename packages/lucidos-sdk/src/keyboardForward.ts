// Forward host keyboard shortcuts out of app iframes.
//
// Apps run inside iframes (AppUiInline.tsx). A keydown fired while an app has
// focus is delivered to the iframe's own document and never reaches the parent —
// so the host shell's global shortcuts (focus/hide a pane, narrow/widen, new
// thread, search, Escape, …) silently die whenever an app is in the focused
// content pane. This forwards the shortcut-shaped chords to the parent via
// postMessage; the host (`useKeyboardShortcuts.ts`) re-dispatches them against
// its registry, so the same chord works whether focus is in the shell or an app.
//
// Only modifier-bearing chords and Escape are forwarded — never plain typing —
// so the app keeps full control of its own text input and the user's keystrokes
// never leak to the parent. The host ignores any forwarded chord that matches no
// shortcut, so over-forwarding a non-shortcut chord (e.g. ⌘C) is harmless.

/** Wire type for a forwarded keydown. Hardcoded as a string literal on the host
 *  side too (`useKeyboardShortcuts.ts`) — same convention as `lucidos:ui:confirm`. */
export const FORWARD_KEYDOWN_TYPE = 'lucidos:keydown';

type ChordSource = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'altKey' | 'key'>;

/** A keydown worth forwarding to the host: it carries a primary modifier
 *  (Cmd/Ctrl) or Alt, or it is Escape. Shift alone never qualifies — no host
 *  shortcut is Shift-only, and a bare Shift+letter is just typing to the app. */
export function isForwardableKeydown(e: ChordSource): boolean {
  return e.metaKey || e.ctrlKey || e.altKey || e.key === 'Escape';
}

export interface ForwardedKeydown {
  type: typeof FORWARD_KEYDOWN_TYPE;
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/** The minimal chord the host matcher reads off a KeyboardEvent. */
export function toForwardedKeydown(e: KeyboardEvent): ForwardedKeydown {
  return {
    type: FORWARD_KEYDOWN_TYPE,
    key: e.key,
    metaKey: e.metaKey,
    ctrlKey: e.ctrlKey,
    shiftKey: e.shiftKey,
    altKey: e.altKey,
  };
}

/**
 * Install the keydown forwarder for the current iframe. Call once on SDK load.
 * Returns a cleanup function that removes the listener.
 *
 * No-op when there is no parent window (SDK loaded at the top level — e.g. a
 * standalone test harness): there is nothing to forward to.
 */
export function installKeyboardForwarding(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const onKeyDown = (e: KeyboardEvent) => {
    if (!isForwardableKeydown(e)) return;
    window.parent.postMessage(toForwardedKeydown(e), '*');
  };
  // Capture phase so the chord is forwarded even if an app handler stops
  // propagation. We never preventDefault — forwarding must not change how the
  // app sees its own keystrokes; the host only ACTS on chords that match a
  // shortcut and ignores the rest.
  window.addEventListener('keydown', onKeyDown, true);
  return () => window.removeEventListener('keydown', onKeyDown, true);
}
