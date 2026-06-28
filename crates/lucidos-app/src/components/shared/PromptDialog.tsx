import { useEffect, useRef } from 'preact/hooks';
import { promptState } from '../../store/store';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';

function close(value: string | null) {
  const state = promptState.peek();
  state.resolve?.(value);
  promptState.value = { visible: false, message: '' };
}

export function PromptDialog() {
  const state = promptState.value;
  const dialogRef = useRef<HTMLDivElement>(null);

  // The native panel webview paints over the dialog; hold it hidden while open.
  useHidePanelWebviewWhile(state.visible);

  useEffect(() => {
    if (!state.visible) return;

    const input = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
    if (input) {
      input.value = state.defaultValue ?? '';
      input.focus();
      input.select();
    }

    function handleKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (e.key === 'Enter') {
        // Buttons handle Enter natively (triggers click). A multiline textarea
        // needs newlines, so it does NOT submit on a bare Enter; a single-line
        // input does.
        if (target?.tagName === 'BUTTON') return;
        if (state.multiline && target?.tagName === 'TEXTAREA') return;
        e.preventDefault();
        const el = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
        close(el?.value ?? '');
        return;
      }
      if (e.key !== 'Tab') return;
      const root = dialogRef.current;
      if (!root) return;
      const focusables = root.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
      );
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('keydown', handleKey);
    };
    // Key on `resolve` (a fresh closure per showPrompt call), not `visible`: a
    // second prompt that REPLACES a visible one keeps visible true→true, so a
    // `[state.visible]` dep would skip re-seeding the uncontrolled input — the
    // new prompt would show the prior one's typed text and not refocus.
  }, [state.resolve]);

  if (!state.visible) return null;

  const okLabel = state.okLabel || 'OK';
  const cancelLabel = state.cancelLabel || 'Cancel';

  function submit() {
    const el = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
    close(el?.value ?? '');
  }

  return (
    <Overlay
      open
      onClose={() => close(null)}
      panelClass="confirm-dialog"
      panelRole="dialog"
      ariaModal
      panelRef={dialogRef}
    >
        {state.title && <h2 class="confirm-title">{state.title}</h2>}
        <p class="confirm-message">{state.message}</p>
        {state.multiline ? (
          <textarea class="prompt-input prompt-textarea" placeholder={state.placeholder} rows={4} />
        ) : (
          <input type="text" class="prompt-input" placeholder={state.placeholder} />
        )}
        <div class="confirm-actions">
          <div class="confirm-actions-right">
            <button class="confirm-btn confirm-btn-cancel" onClick={() => close(null)}>
              {cancelLabel}
            </button>
            <button class="confirm-btn confirm-btn-ok-default" onClick={submit}>
              {okLabel}
            </button>
          </div>
        </div>
    </Overlay>
  );
}
