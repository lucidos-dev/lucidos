import { useEffect, useRef } from 'preact/hooks';
import { promptState } from '../../store/store';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { DialogMessage } from './DialogMessage';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { trapDialogTab } from './dialogFocusTrap';
import { dialogOwnsKey } from './dialogKeyScope';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { isImeComposingKey } from '../../utils/ime';

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
      // A keystroke aimed at another open overlay is not ours to submit: a
      // confirm can sit on top of this prompt, and this listener would
      // otherwise close it with the input's text. See dialogOwnsKey.
      if (!dialogOwnsKey(target, dialogRef.current)) return;
      if (e.key === 'Enter') {
        if (!promptEnterSubmits(e, target?.tagName, state.multiline)) return;
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
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog protected-surface"
      panelRole="dialog"
      ariaModal
      panelRef={dialogRef}
    >
      {state.title && <SurfaceHead title={state.title} />}
      <div class="surface-body dialog-body" tabIndex={-1}>
        <DialogMessage message={state.message} />
        {/* The answer is free-form natural language (the dialog is driven by the
            LLM's ask-the-user payload), so both branches are prose fields. */}
        {/* `onInput` records the draft (see openPromptDraft) and nothing else:
            the field stays uncontrolled, so typing still costs no re-render. */}
        {state.multiline ? (
          <textarea class="prompt-input prompt-textarea" placeholder={state.placeholder} rows={4} onInput={recordDraft} {...PROSE_TEXT_ATTRS} />
        ) : (
          <input type="text" class="prompt-input" placeholder={state.placeholder} onInput={recordDraft} {...PROSE_TEXT_ATTRS} />
        )}
      </div>
      <div class="surface-foot">
        <button class="action-btn action-btn-secondary" data-role="prompt-cancel" onClick={() => close(null)}>
          {cancelLabel}
        </button>
        <button class="action-btn" data-role="prompt-ok" onClick={submit}>
          {okLabel}
        </button>
      </div>
    </Overlay>
  );
}
