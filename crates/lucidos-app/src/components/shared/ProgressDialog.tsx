import type { Ref, VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { activeProgressDialog } from '../../store/store';
import type { ProgressDialogState } from '../../store/types';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { DialogMessage } from './DialogMessage';
import { Overlay } from './Overlay';
import { trapDialogTab } from './dialogFocusTrap';
import { progressFillWidth } from './progressBar';

/** The modal for an operation that takes the workspace away and brings it back.
 *
 *  Third surface in the taxonomy, beside the toast and the banner: a toast is
 *  for something ignorable and a banner is for a condition you can work around,
 *  and this is for neither. Nothing behind it is usable while it runs, so it
 *  says so instead of leaving the user to discover it.
 *
 *  NOT dismissable, and deliberately so. The operation continues whatever the
 *  user presses, so an X would hide the only account of what is happening. The
 *  one control is Cancel, and only while cancelling is still possible.
 *
 *  See docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md. */

/** Pure markup, hook-free so the gallery and the tests can call it directly.
 *  The `backupReminderBody` / `connectionBannerBody` idiom. */
export function progressDialogBody(props: {
  state: ProgressDialogState;
  panelRef?: Ref<HTMLDivElement>;
}): VNode {
  const { title, message, progress, cancel } = props.state;
  return (
    // `tabIndex={-1}` makes the panel programmatically focusable without
    // becoming a Tab stop. A committed phase has no Cancel, so it has no
    // focusable control at all. Focus goes here then, leaving whatever button
    // opened the dialog.
    <div ref={props.panelRef} class="progress-dialog-body" tabIndex={-1}>
      <h2 class="confirm-title">{title}</h2>
      <DialogMessage message={message} />
      {/* Determinate only where the operation has an honest percentage. A
          download does; a service restart does not, and a bar that invents one
          is a lie the user waits on. `progressFillWidth` clamps, so a bad
          fraction paints an empty track rather than running past its box. */}
      {progress != null && (
        <div class="progress-bar progress-dialog-bar">
          <div class="progress-bar-fill" style={{ width: progressFillWidth(progress) }} />
        </div>
      )}
      {progress == null && <div class="mini-spinner progress-dialog-spinner" />}
      {cancel && (
        <div class="confirm-actions">
          <div class="confirm-actions-right">
            <button class="confirm-btn confirm-btn-cancel" onClick={cancel.onClick}>
              {cancel.label}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** The container: owns the slot and the overlay, so the body stays pure.
 *
 *  `onClose` returns false, which is the `<Overlay>` contract's way of saying
 *  the dismiss was a no-op. That keeps the user's click from being swallowed on
 *  a surface that cannot be dismissed anyway. */
export function ProgressDialog() {
  const state = activeProgressDialog.value;
  const panelRef = useRef<HTMLDivElement>(null);

  useHidePanelWebviewWhile(state.visible);

  // Take focus, and keep it. Without this the dialog declares `aria-modal`
  // while focus stays on whatever button opened it. Enter then re-fires that
  // button from behind the modal, and Tab walks the page underneath.
  useEffect(() => {
    if (!state.visible) return;
    const panel = panelRef.current;
    if (!panel) return;
    const cancelBtn = panel.querySelector<HTMLButtonElement>('.confirm-btn-cancel');
    (cancelBtn ?? panel).focus();

    function handleKey(e: KeyboardEvent) {
      if (e.key !== 'Tab') return;
      const root = panelRef.current;
      // No Cancel means nothing inside is tabbable, and `trapDialogTab` would
      // let Tab fall through to the page. Swallow it instead, so focus cannot
      // leave a modal the user has no way to answer yet.
      if (!root?.querySelector('button:not([disabled])')) {
        e.preventDefault();
        return;
      }
      trapDialogTab(e, root);
    }
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
    // Whether a Cancel EXISTS, never the object itself. Each phase builds a
    // fresh one. Depending on its identity would re-run this every tick and
    // yank focus back to the button several times per operation.
  }, [state.visible, state.cancel !== undefined]);

  if (!state.visible) return null;

  return (
    <Overlay
      open
      onClose={() => false}
      panelClass="confirm-dialog progress-dialog"
      panelRole="dialog"
      ariaModal
    >
      {progressDialogBody({ state, panelRef })}
    </Overlay>
  );
}
