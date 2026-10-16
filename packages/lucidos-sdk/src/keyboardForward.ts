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
// Only modifier-bearing chords, Escape and the F-keys are forwarded, never plain
// typing. The app keeps full control of its own text input, and the user's
// keystrokes never leak to the parent. The host ignores any forwarded chord that matches no
// shortcut, so over-forwarding a non-shortcut chord (e.g. ⌘C) is harmless.
//
// The host pushes its current bindings on the `keybindings` channel. A chord
// bound to a host shortcut has its browser default cancelled here, so ⌘P
// opens file search and not the print dialog. The app's own handlers still
// receive the key, already marked `defaultPrevented`.

import { onHostPush } from './_bridge';

/** Wire type for a forwarded keydown. Hardcoded as a string literal on the host
 *  side too (`useKeyboardShortcuts.ts`) — same convention as `lucidos:ui:confirm`. */
export const FORWARD_KEYDOWN_TYPE = 'lucidos:keydown';

/** Push channel carrying the host's shortcut bindings. The host imports this
 *  constant (`store/actions/app-keybindings.ts`), so both ends speak one name. */
export const KEYBINDINGS_CHANNEL = 'keybindings';

/** One host binding, in the host registry's shape (`utils/shortcuts.ts`). */
interface HostBinding {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

let hostBindings: readonly HostBinding[] = [];

function isHostBinding(v: unknown): v is HostBinding {
  const b = v as HostBinding | null;
  return !!b && typeof b.mod === 'boolean' && typeof b.shift === 'boolean'
    && typeof b.alt === 'boolean' && typeof b.key === 'string';
}

/** Take the bindings the host pushed. Anything malformed is dropped, so a bad
 *  push can only mean fewer cancelled defaults, never a thrown handler. */
export function adoptHostBindings(data: unknown): void {
  const list = (data as { bindings?: unknown } | null)?.bindings;
  hostBindings = Array.isArray(list) ? list.filter(isHostBinding) : [];
}

/** The host registry's key rule (`normalizeKey` in `utils/shortcuts.ts`). */
function normalizeKey(key: string): string {
  if (key === '+') return '=';
  return key.length === 1 ? key.toLowerCase() : key;
}

/** Whether a keydown is bound to a host shortcut, by the host's own match rule:
 *  Cmd and Ctrl both count as `mod`. */
export function matchesHostBinding(
  e: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>,
  bindings: readonly HostBinding[] = hostBindings,
): boolean {
  const key = normalizeKey(e.key);
  return bindings.some((b) => (e.metaKey || e.ctrlKey) === b.mod
    && e.shiftKey === b.shift && e.altKey === b.alt && key === b.key);
}

type ChordSource = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'altKey' | 'key'>
  & Partial<Pick<KeyboardEvent, 'shiftKey' | 'target'>>;

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

function isEditable(target: EventTarget | null | undefined): boolean {
  const el = target as HTMLElement | null | undefined;
  return !!el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName ?? ''));
}

/** A keydown worth forwarding to the host: it carries a primary modifier
 *  (Cmd/Ctrl) or Alt, or it is Escape or an F-key (F2 renames a thread). Shift
 *  alone never qualifies: a bare Shift+letter is just typing to the app. Nor
 *  does Ctrl+letter in a Mac text field, where it edits text (Ctrl+K). */
export function isForwardableKeydown(e: ChordSource, mac: boolean = IS_MAC): boolean {
  const macTextEdit = mac && e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey
    && /^[a-z]$/i.test(e.key) && isEditable(e.target);
  if (macTextEdit) return false;
  return e.metaKey || e.ctrlKey || e.altKey || e.key === 'Escape' || /^F\d{1,2}$/.test(e.key);
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
  const offBindings = onHostPush(KEYBINDINGS_CHANNEL, adoptHostBindings);
  const onKeyDown = (e: KeyboardEvent) => {
    if (!isForwardableKeydown(e)) return;
    if (matchesHostBinding(e)) e.preventDefault();
    window.parent.postMessage(toForwardedKeydown(e), '*');
  };
  // Capture phase so the chord is forwarded even if an app handler stops
  // propagation. Only a host-bound chord has its browser default cancelled.
  // The app's own handlers still receive every keystroke.
  window.addEventListener('keydown', onKeyDown, true);
  return () => {
    window.removeEventListener('keydown', onKeyDown, true);
    offBindings();
  };
}
