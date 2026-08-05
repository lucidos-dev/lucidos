import { useEffect, useRef } from 'preact/hooks';
import { promptState } from '../../store/store';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { Overlay } from './Overlay';
import { trapDialogTab } from './dialogFocusTrap';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';

/** What the reader has typed into the prompt that is currently open, kept
 *  outside the component so it survives a REMOUNT of the same prompt.
 *
 *  The input is deliberately uncontrolled (no re-render per keystroke), so its
 *  text lives only in the DOM node, and a remount seeds a fresh node from
 *  `defaultValue`. That is right when a new prompt replaces this one and wrong
 *  when it is the same prompt landing in a new place: the overlay layer
 *  re-parents into and out of a fullscreen app panel (`OverlayLayer`), which
 *  remounts everything under it, so leaving fullscreen mid-answer used to reset
 *  what the reader had written.
 *
 *  Keyed on the prompt's `resolve` closure, which is fresh per `showPrompt`
 *  call: same closure means the same question, so the draft applies; a different
 *  one is a different question and seeds from its own `defaultValue`. Cleared on
 *  close so a later prompt can never inherit it. */
let openPromptDraft: { resolve: unknown; value: string } | null = null;

/** What the input is seeded with on (re)mount: the draft when it belongs to
 *  THIS prompt, else the prompt's own default. Pure, and exported for the test
 *  that pins both halves. */
export function promptInputSeed(
  draft: { resolve: unknown; value: string } | null,
  resolve: unknown,
  defaultValue: string | undefined,
): string {
  return (draft && draft.resolve === resolve ? draft.value : defaultValue) ?? '';
}

function close(value: string | null) {
  const state = promptState.peek();
  openPromptDraft = null;
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
      input.value = promptInputSeed(openPromptDraft, state.resolve, state.defaultValue);
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
      trapDialogTab(e, dialogRef.current);
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

  function recordDraft(e: Event) {
    const el = e.currentTarget as HTMLInputElement | HTMLTextAreaElement;
    openPromptDraft = { resolve: state.resolve, value: el.value };
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
        {/* The answer is free-form natural language (the dialog is driven by the
            LLM's ask-the-user payload), so both branches are prose fields. */}
        {/* `onInput` records the draft (see openPromptDraft) and nothing else:
            the field stays uncontrolled, so typing still costs no re-render. */}
        {state.multiline ? (
          <textarea class="prompt-input prompt-textarea" placeholder={state.placeholder} rows={4} onInput={recordDraft} {...PROSE_TEXT_ATTRS} />
        ) : (
          <input type="text" class="prompt-input" placeholder={state.placeholder} onInput={recordDraft} {...PROSE_TEXT_ATTRS} />
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
