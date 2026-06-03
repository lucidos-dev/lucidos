import { useEffect } from 'preact/hooks';
import { unfocusThread } from '../store/actions/threads';
import { focusPromptNow } from '../components/chat/promptFocus';
import { searchEverywhereOpen } from '../store/store';
import { isTextInput } from '../utils/dom';
import { dismissTopOverlay } from '../store/overlayStack';
import { runCloseCascade } from '../store/actions/threadActions';
import { matchShortcut } from '../store/actions/keybindings';
import type { ShortcutId } from '../utils/shortcuts';
import { adjustUiScale, resetUiScale, scaleModalOpen, dismissScaleModal } from '../components/shared/scaleModalState';
import { UI_SCALE_STEP } from '../store/actions/preferences';
import { isMobile } from '../utils/viewport';

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

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Registry-driven shortcuts (Search, New thread, Close cascade, Zoom …).
      // Each dispatches against the user's CURRENT binding, so customizing a
      // shortcut in Settings just works. The single-key 'c'/'t' shortcuts were
      // dropped — bare letters now fall through to type-to-focus below.
      const id = matchShortcut(e);
      if (id) {
        e.preventDefault();
        SHORTCUT_ACTIONS[id]();
        return;
      }

      // Auto-focus prompt on typing (desktop only, printable characters).
      // The keydown targeted <body>, so the browser won't insert the char
      // into the newly-focused textarea — insert it manually.
      if (!isMobile() && !e.isComposing && !isTextInput(e.target) && !e.metaKey && !e.ctrlKey && !e.altKey && e.key.length === 1) {
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

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('keyup', handleKeyUp);
    document.addEventListener('keydown', handleEscapeCapture, true);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('keyup', handleKeyUp);
      document.removeEventListener('keydown', handleEscapeCapture, true);
    };
  }, []);
}
