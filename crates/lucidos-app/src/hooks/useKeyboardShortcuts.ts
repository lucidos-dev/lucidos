import { useEffect } from 'preact/hooks';
import { unfocusThread } from '../store/actions/threads';
import { focusPromptNow } from '../components/chat/promptFocus';
import { searchEverywhereOpen, focusedPane } from '../store/store';
import { isTextInput } from '../utils/dom';
import { dismissTopOverlay, overlayStack } from '../store/overlayStack';
import { runCloseCascade } from '../store/actions/threadActions';
import { matchShortcut } from '../store/actions/keybindings';
import type { ShortcutId } from '../utils/shortcuts';
import { isKnownAppFrame } from '../utils/appFrame';
import { adjustUiScale, resetUiScale, scaleModalOpen, dismissScaleModal } from '../components/shared/scaleModalState';
import { UI_SCALE_STEP } from '../store/actions/preferences';
import { isMobile } from '../utils/viewport';
import {
  toggleThreadPane, toggleContentPane,
  focusOrToggleThreadDrawer, toggleMaximizeFocusedPaneGroup,
  stepThreadPaneWidth, stepThreadDrawerWidth, resetPaneLayout,
} from '../store/actions/pane';
import { seedDrawerHighlight } from '../components/drawer/ThreadDrawer';
import { handlePaneTab } from '../components/layout/paneFocus';
import { historyBack, historyForward } from '../store/actions/focused-pane-history';

function startNewThread() {
  unfocusThread();
  requestAnimationFrame(() => {
    focusPromptNow();
  });
}

/** What each registry shortcut does when its (current, possibly-customized)
 *  binding fires. The registry (`utils/shortcuts.ts`) owns the keys; this map
 *  owns the behavior — so rebinding a shortcut in Settings takes effect here
 *  with no code change. */
const SHORTCUT_ACTIONS: Record<ShortcutId, () => void> = {
  newThread: startNewThread,
  closeThread: () => void runCloseCascade(),
  searchEverywhere: () => { searchEverywhereOpen.value = !searchEverywhereOpen.value; },
  historyBack: () => historyBack(),
  historyForward: () => historyForward(),
  toggleThreadDrawer: () => { if (focusOrToggleThreadDrawer()) seedDrawerHighlight(); },
  toggleThreadPane,
  toggleContentPane,
  maximizePaneGroup: toggleMaximizeFocusedPaneGroup,
  narrowThreadPane: () => stepThreadPaneWidth(-1),
  widenThreadPane: () => stepThreadPaneWidth(1),
  narrowThreadDrawer: () => stepThreadDrawerWidth(-1),
  widenThreadDrawer: () => stepThreadDrawerWidth(1),
  resetPaneLayout,
  zoomIn: () => adjustUiScale(UI_SCALE_STEP),
  zoomOut: () => adjustUiScale(-UI_SCALE_STEP),
  zoomReset: () => resetUiScale(),
};

/** Non-destructive Escape policy, in priority order:
 *  1. dismiss the top registered overlay (modal / confirm / pseudo-fullscreen),
 *  2. else, if the focused text input manages its own Escape (`data-escape-self`),
 *     leave focus alone so its keydown handler can run — used by inputs where a
 *     blur commits work (e.g. the thread-title editor, where blur saves a rename
 *     so a blur-on-Escape would SAVE instead of cancel),
 *  3. else blur a focused text input (the universal "Esc defocuses" gesture),
 *  4. else no-op — Escape NEVER touches the focused thread or discards work.
 *  Returns which branch fired so the caller can preventDefault/stopPropagation
 *  appropriately. Exported for unit testing. */
export function dispatchEscape(
  active: Element | null,
): 'dismissed' | 'self-managed' | 'blurred' | 'noop' {
  if (dismissTopOverlay()) return 'dismissed';
  if (isTextInput(active)) {
    if ((active as HTMLElement).hasAttribute?.('data-escape-self')) return 'self-managed';
    (active as HTMLElement).blur();
    return 'blurred';
  }
  return 'noop';
}

/** Wire type for a keydown the SDK forwards out of an app iframe — must match
 *  `FORWARD_KEYDOWN_TYPE` in `packages/lucidos-sdk/src/keyboardForward.ts`.
 *  Hardcoded both sides, same convention as `lucidos:ui:confirm`. */
const FORWARDED_KEYDOWN_TYPE = 'lucidos:keydown';

type ChordLike = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>;

/** Whether a bare keydown that bubbled to <body> should type-to-focus the prompt
 *  textarea (desktop convenience: start typing anywhere → land in the prompt).
 *  False when an overlay is open — a dropdown / modal / popover owns keystrokes
 *  while up, so typing must search the dropdown (its auto-focused filter), never
 *  leak into the prompt textarea behind the inert UI. Also false on mobile, for
 *  IME composition, when a text input already has focus, with a modifier held,
 *  or for non-printable keys. Pure — exported for unit testing. */
export function shouldTypeToFocusPrompt(
  e: Pick<KeyboardEvent, 'isComposing' | 'metaKey' | 'ctrlKey' | 'altKey' | 'key' | 'target'>,
  opts: { mobile: boolean; overlayOpen: boolean },
): boolean {
  return !opts.mobile
    && !opts.overlayOpen
    && !e.isComposing
    && !isTextInput(e.target)
    && !e.metaKey && !e.ctrlKey && !e.altKey
    && e.key.length === 1;
}

/** Classify a chord forwarded from an app iframe (its keydowns never reach the
 *  host document, so the SDK forwards shortcut-shaped chords via postMessage).
 *  Returns the matching shortcut id, `'escape'` for the Escape policy, or `null`
 *  when the chord matches no shortcut (the host ignores it). Pure — exported for
 *  testing; the dispatch (running the action) lives in the message handler. */
export function classifyForwardedChord(chord: ChordLike): ShortcutId | 'escape' | null {
  const id = matchShortcut(chord);
  if (id) return id;
  if (chord.key === 'Escape') return 'escape';
  return null;
}

/** Run a chord forwarded from an app iframe against the host registry. The
 *  forward itself is proof the content pane is the focused pane — the app lives
 *  there and just received the keydown — but pointer events inside the iframe
 *  never reach the host's `focusPane('content')` handler, so `focusedPane` is
 *  stale. Reconcile it BEFORE dispatching, or the three-state pane toggles read
 *  the wrong state: notably `toggleContentPane` (⌘⇧3) would "focus" the
 *  already-focused pane (a no-op) instead of CLOSING it. Exported for testing. */
export function dispatchForwardedChord(chord: ChordLike): void {
  const result = classifyForwardedChord(chord);
  if (result === null) return;
  focusedPane.value = 'content';
  if (result === 'escape') {
    // Focus is in the iframe, so activeElement is the <iframe>. The policy
    // dismisses any open host overlay (e.g. an SDK confirm) and otherwise
    // no-ops — it never touches the focused thread.
    dispatchEscape(document.activeElement);
  } else {
    SHORTCUT_ACTIONS[result]();
  }
}

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Registry-driven shortcuts (Search, New thread, Close cascade, Zoom …).
      // Each dispatches against the user's CURRENT binding, so customizing a
      // shortcut in Settings just works. The single-key 'c'/'t' shortcuts were
      // dropped — bare letters now fall through to type-to-focus below.
      // Runs BEFORE the Tab trap so a user who rebinds a shortcut onto Tab
      // still gets it — the trap only claims Tab presses that match no binding.
      const id = matchShortcut(e);
      if (id) {
        e.preventDefault();
        SHORTCUT_ACTIONS[id]();
        return;
      }

      // Per-pane Tab trap: while focus is inside a pane (and no overlay is open),
      // Tab/Shift+Tab cycle within that pane. Switch panes with the ⌘⇧ pane
      // shortcuts or a click. Falls through to default Tab when focus is outside
      // any pane or an overlay owns it.
      if (e.key === 'Tab') {
        if (handlePaneTab(e)) e.preventDefault();
        return;
      }

      // Auto-focus prompt on typing (desktop only, printable characters).
      // Skipped while an overlay is open so typing searches the dropdown /
      // modal instead of leaking into the prompt textarea behind it.
      // The keydown targeted <body>, so the browser won't insert the char
      // into the newly-focused textarea — insert it manually.
      if (shouldTypeToFocusPrompt(e, { mobile: isMobile(), overlayOpen: overlayStack.value.length > 0 })) {
        focusPromptNow();
        e.preventDefault();
        document.execCommand('insertText', false, e.key);
      }
    }

    function handleKeyUp(e: KeyboardEvent) {
      // Dismiss scale panel when Cmd/Ctrl is released
      if ((e.key === 'Meta' || e.key === 'Control') && scaleModalOpen.value) {
        dismissScaleModal();
      }
    }

    // Escape is dispatched in the CAPTURE phase so it runs before any element's
    // own keydown handler (e.g. the prompt textarea's blur), giving the central
    // policy first say: dismiss the top overlay, else blur, else no-op. When an
    // overlay is dismissed we stop propagation so nothing downstream double-acts.
    function handleEscapeCapture(e: KeyboardEvent) {
      if (e.key !== 'Escape') return;
      const result = dispatchEscape(document.activeElement);
      if (result === 'dismissed') {
        e.preventDefault();
        e.stopPropagation();
      } else if (result === 'blurred') {
        e.preventDefault();
      }
      // 'self-managed' / 'noop' fall through untouched: the focused element's
      // own keydown Escape handler gets the event (and may preventDefault).
    }

    // Keydowns fired inside an app iframe never reach this document, so the
    // shortcuts above silently die whenever an app has focus in the content
    // pane. The SDK forwards shortcut-shaped chords up via postMessage
    // (keyboardForward.ts); re-dispatch them against the same registry here.
    function handleForwardedKeydown(e: MessageEvent) {
      const data = e.data as {
        type?: unknown; key?: unknown;
        metaKey?: unknown; ctrlKey?: unknown; shiftKey?: unknown; altKey?: unknown;
      } | null;
      if (!data || typeof data !== 'object' || data.type !== FORWARDED_KEYDOWN_TYPE) return;
      if (typeof data.key !== 'string') return;
      // Only honor chords from a current app iframe — a nested embed/ad inside an
      // app, or an unrelated window, must not drive host shortcuts.
      if (!isKnownAppFrame(e.source)) return;
      dispatchForwardedChord({
        key: data.key,
        metaKey: data.metaKey === true,
        ctrlKey: data.ctrlKey === true,
        shiftKey: data.shiftKey === true,
        altKey: data.altKey === true,
      });
    }

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('keyup', handleKeyUp);
    document.addEventListener('keydown', handleEscapeCapture, true);
    window.addEventListener('message', handleForwardedKeydown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('keyup', handleKeyUp);
      document.removeEventListener('keydown', handleEscapeCapture, true);
      window.removeEventListener('message', handleForwardedKeydown);
    };
  }, []);
}
