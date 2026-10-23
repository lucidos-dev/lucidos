import { useEffect, useRef } from 'preact/hooks';
import { confirmState, settleConfirm } from '../../store/store';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { DialogMessage } from './DialogMessage';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { dialogOwnsKey } from './dialogKeyScope';

export function ConfirmDialog() {
  const state = confirmState.value;
  const dialogRef = useRef<HTMLDivElement>(null);
  const okBtnRef = useRef<HTMLButtonElement>(null);

  // The native panel webview paints over the dialog; hold it hidden while open.
  useHidePanelWebviewWhile(state.visible);

  useEffect(() => {
    if (!state.visible) return;

    okBtnRef.current?.focus();

    function handleKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      // A keystroke aimed at another open overlay is not ours to answer: a
      // prompt can sit on top of this confirm, and this listener would
      // otherwise resolve `true` on the Enter that submits it. See dialogOwnsKey.
      if (!dialogOwnsKey(target, dialogRef.current)) return;
      if (e.key === 'Enter') {
        // Buttons handle Enter natively (triggers click); textareas need newlines.
        if (target?.tagName === 'BUTTON' || target?.tagName === 'TEXTAREA') return;
        e.preventDefault();
        settleConfirm(true);
      }
    }
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('keydown', handleKey);
    };
    // Key on `resolve` (a fresh closure per showConfirm call), not `visible`.
    // A second confirm REPLACES a visible one, keeping visible true→true, so a
    // `[state.visible]` dep would skip the focus. The caret would stay on the
    // previous dialog's button. Same reason as `PromptDialog`.
  }, [state.resolve]);

  if (!state.visible) return null;

  const cancelLabel = state.cancelLabel || 'Cancel';
  const choices = state.choices;

  return (
    <Overlay
      open
      onClose={() => settleConfirm(state.acknowledge === true)}
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog protected-surface"
      panelRole="dialog"
      ariaModal
      panelRef={dialogRef}
    >
      {state.title && <SurfaceHead title={state.title} />}
      <div class="surface-body dialog-body" tabIndex={-1}>
        <DialogMessage message={state.message} />
        {state.details && (
          // `tabIndex={-1}`, so Chrome leaves the list out of the Tab order. A
          // long one overflows, and Chrome promotes an overflowing scroller
          // with no focusable child to a Tab stop. The overlay Tab rule cycles
          // the two buttons and never names it, so the promotion only ever painted
          // the browser's ring around the list. See `.confirm-details:focus`.
          <div class="confirm-details" tabIndex={-1}>
            {state.details.intro && <p class="confirm-details-intro">{state.details.intro}</p>}
            {state.details.groups.map((g, gi) => (
              <div class="confirm-details-group" key={gi}>
                <div class="confirm-details-header">{g.header}</div>
                {g.items.length > 0 && (
                  <ul class="confirm-details-list">
                    {g.items.map((item, ii) => <li key={ii}>{item}</li>)}
                  </ul>
                )}
              </div>
            ))}
          </div>
        )}
        {choices && (
          <div class="confirm-choices">
            <button ref={okBtnRef} class="confirm-choice confirm-choice-primary" data-role="confirm-ok" onClick={() => settleConfirm(true)}>
              <strong>{state.okLabel}</strong>
              <span>{choices.okDetail}</span>
            </button>
            <button
              class="confirm-choice"
              data-role="confirm-extra"
              onClick={() => {
                choices.alternative.onClick();
                settleConfirm(false);
              }}
            >
              <strong>{choices.alternative.label}</strong>
              <span>{choices.alternative.detail}</span>
            </button>
          </div>
        )}
      </div>
      <div class="surface-foot">
        {/* An acknowledgement has no Cancel: there is nothing to decline, and
            a second button meaning the same as the first reads as a choice
            the user does not have. Escape and an outside click still work,
            and `settleConfirm(true)` below is why they mean "read it". */}
        {!state.acknowledge && state.extraAction && (
          <button
            class="action-btn action-btn-secondary confirm-extra"
            data-role="confirm-extra"
            onClick={() => {
              state.extraAction!.onClick();
              settleConfirm(false);
            }}
          >
            {state.extraAction.label}
          </button>
        )}
        {!state.acknowledge && (
          <button class="action-btn action-btn-secondary" data-role="confirm-cancel" onClick={() => settleConfirm(false)}>
            {cancelLabel}
          </button>
        )}
        {!choices && (
          <button
            ref={okBtnRef}
            class={`action-btn${state.variant === 'danger' ? ' action-btn-danger' : ''}`}
            data-role="confirm-ok"
            onClick={() => settleConfirm(true)}
          >
            {state.okLabel}
          </button>
        )}
      </div>
    </Overlay>
  );
}
