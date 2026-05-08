import { useEffect } from 'preact/hooks';
import { unfocusThread } from '../store/actions/threads';
import { focusPromptNow } from '../components/chat/promptFocus';
import { searchEverywhereOpen } from '../store/store';
import { toggleThreads } from '../store/actions/pane';
import { isTextInput } from '../utils/dom';
import { adjustUiScale, resetUiScale, scaleModalOpen, dismissScaleModal } from '../components/shared/ScaleModal';
import { UI_SCALE_STEP } from '../store/actions/preferences';
import { isMobile } from '../utils/viewport';

function startNewThread() {
  unfocusThread();
  requestAnimationFrame(() => {
    focusPromptNow();
  });
}

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Cmd/Ctrl+K: toggle Search Everywhere (works everywhere)
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        searchEverywhereOpen.value = !searchEverywhereOpen.value;
        return;
      }

      // Cmd/Ctrl+Shift+O: new thread (works everywhere)
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === 'O') {
        e.preventDefault();
        startNewThread();
        return;
      }

      // 'c': new thread (only when not in a text input)
      if (e.key === 'c' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey && !isTextInput(e.target)) {
        e.preventDefault();
        startNewThread();
        return;
      }

      // 't': toggle threads panel (only when not in a text input)
      if (e.key === 't' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey && !isTextInput(e.target)) {
        e.preventDefault();
        toggleThreads();
        return;
      }

      // Cmd/Ctrl + '=' (plus) or Cmd/Ctrl + '-' (minus): UI scale
      if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        if (e.key === '=' || e.key === '+') {
          e.preventDefault();
          adjustUiScale(UI_SCALE_STEP);
          return;
        }
        if (e.key === '-') {
          e.preventDefault();
          adjustUiScale(-UI_SCALE_STEP);
          return;
        }
        if (e.key === '0') {
          e.preventDefault();
          resetUiScale();
          return;
        }
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

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('keyup', handleKeyUp);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('keyup', handleKeyUp);
    };
  }, []);
}
