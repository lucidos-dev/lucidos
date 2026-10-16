import { useEffect, useRef, useState } from 'preact/hooks';
import { promptState, settlePrompt, showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { DialogMessage } from './DialogMessage';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { dialogOwnsKey } from './dialogKeyScope';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { isImeComposingKey } from '../../utils/ime';

/** Whether this keydown submits the prompt. Buttons answer Enter themselves,
 *  and a multiline textarea keeps Enter for newlines. An Enter that commits an
 *  IME candidate belongs to the IME, so it never submits half-converted text.
 *  Pure, and exported for testing. */
export function promptEnterSubmits(
  e: Pick<KeyboardEvent, 'key' | 'isComposing' | 'keyCode'>,
  targetTag: string | undefined,
  multiline: boolean | undefined,
): boolean {
  if (e.key !== 'Enter' || isImeComposingKey(e)) return false;
  if (targetTag === 'BUTTON') return false;
  return !(multiline && targetTag === 'TEXTAREA');
}

function close(value: string | null) {
  settlePrompt(value);
}

export function PromptDialog() {
  const state = promptState.value;
  const dialogRef = useRef<HTMLDivElement>(null);
  const [suggesting, setSuggesting] = useState(false);

  // The native panel webview paints over the dialog; hold it hidden while open.
  useHidePanelWebviewWhile(state.visible);

  useEffect(() => {
    setSuggesting(false);
    if (!state.visible) return;

    const input = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
    if (input) {
      input.value = state.defaultValue ?? '';
      input.focus();
      input.select();
    }

    function handleKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      // A keystroke aimed at another open overlay is not ours to submit: a
      // confirm can sit on top of this prompt, and this listener would
      // otherwise close it with the input's text. See dialogOwnsKey.
      if (!dialogOwnsKey(target, dialogRef.current)) return;
      if (e.key === 'Enter') {
        if (!promptEnterSubmits(e, target?.tagName, state.multiline)) return;
        e.preventDefault();
        const el = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
        // A suggestion in flight locks the field, and Enter waits with it.
        if (el?.readOnly) return;
        close(el?.value ?? '');
      }
    }
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('keydown', handleKey);
    };
    // Key on `resolve` (a fresh closure per showPrompt call), not `visible`: a
    // second prompt that REPLACES a visible one keeps visible true→true, so a
    // `[state.visible]` dep would skip re-seeding the uncontrolled input — the
    // new prompt would show the prior one's typed text and not refocus. It
    // also drops a replaced prompt's suggestion lock.
  }, [state.resolve]);

  if (!state.visible) return null;

  const okLabel = state.okLabel || 'OK';
  const cancelLabel = state.cancelLabel || 'Cancel';

  function submit() {
    if (suggesting) return;
    const el = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
    close(el?.value ?? '');
  }

  /** Locks the field while the suggestion loads, then fills it. A result for
   *  a prompt that has since closed or been replaced is dropped. */
  async function suggest(run: () => Promise<string | null>) {
    const resolve = state.resolve;
    const isCurrent = () => promptState.peek().resolve === resolve;
    setSuggesting(true);
    try {
      const value = await run();
      if (value === null || !isCurrent()) return;
      const el = dialogRef.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>('.prompt-input');
      if (!el) return;
      el.value = value;
      el.focus();
      el.select();
    } catch (err) {
      showToast(`Could not suggest an answer: ${errorDetail(err)}`, 'error');
    } finally {
      if (isCurrent()) setSuggesting(false);
    }
  }

  const runSuggest = state.suggest;

  return (
    <Overlay
      open
      onClose={() => close(null)}
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog protected-surface"
      panelRole="dialog"
      ariaModal
      panelRef={dialogRef}
    >
      {state.title && <SurfaceHead title={state.title} />}
      <div class="surface-body dialog-body" tabIndex={-1}>
        {runSuggest ? (
          <div class="prompt-message-row">
            <div class="prompt-message"><DialogMessage message={state.message} /></div>
            <SuggestLink suggesting={suggesting} onClick={() => { void suggest(runSuggest); }} />
          </div>
        ) : (
          <DialogMessage message={state.message} />
        )}
        {/* The answer is free-form natural language (the dialog is driven by the
            LLM's ask-the-user payload), so both branches are prose fields. */}
        {state.multiline ? (
          <textarea class="prompt-input prompt-textarea" placeholder={state.placeholder} rows={4} readOnly={suggesting} aria-busy={suggesting} {...PROSE_TEXT_ATTRS} />
        ) : (
          <input type="text" class="prompt-input" placeholder={state.placeholder} readOnly={suggesting} aria-busy={suggesting} {...PROSE_TEXT_ATTRS} />
        )}
      </div>
      <div class="surface-foot">
        <button class="action-btn action-btn-secondary" data-role="prompt-cancel" onClick={() => close(null)}>
          {cancelLabel}
        </button>
        <button class="action-btn" data-role="prompt-ok" disabled={suggesting} onClick={submit}>
          {okLabel}
        </button>
      </div>
    </Overlay>
  );
}

/** The prompt's Suggest link. It spins while the suggestion loads, and stays
 *  in the focus order meanwhile, so `aria-disabled` rather than `disabled`. */
function SuggestLink({ suggesting, onClick }: { suggesting: boolean; onClick: () => void }) {
  return (
    <button type="button" class="accent-link prompt-suggest" data-role="prompt-suggest"
      aria-disabled={suggesting} aria-busy={suggesting}
      onClick={() => { if (!suggesting) onClick(); }}>
      {suggesting && <span class="mini-spinner" aria-hidden="true" />}
      Suggest
    </button>
  );
}
