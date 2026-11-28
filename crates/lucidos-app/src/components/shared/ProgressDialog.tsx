import type { VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { activeProgressDialog } from '../../store/store';
import type { ProgressDialogState } from '../../store/types';
import { useHidePanelWebviewWhile } from '../../hooks/useHidePanelWebviewWhile';
import { DialogMessage } from './DialogMessage';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { progressFillWidth } from './progressBar';


export function progressDialogBody(props: {
  state: ProgressDialogState;
}): VNode {
  const { title, message, progress, cancel } = props.state;
  return (
    <>
      <SurfaceHead
        title={title}
        icon={
          progress == null ? (
            <span class="surface-icon" aria-hidden="true"><span class="mini-spinner" /></span>
          ) : undefined
        }
      />
      <div class="surface-body progress-dialog-readout">
        <DialogMessage message={message} />
        {progress != null && (
          <div class="progress-bar progress-dialog-bar">
            <div class="progress-bar-fill" style={{ width: progressFillWidth(progress) }} />
          </div>
        )}
      </div>
      {cancel && (
        <div class="surface-foot">
          <button class="action-btn action-btn-secondary" data-role="progress-cancel" onClick={cancel.onClick}>
            {cancel.label}
          </button>
        </div>
      )}
    </>
  );
}

export function ProgressDialog() {
  const state = activeProgressDialog.value;
  const panelRef = useRef<HTMLDivElement>(null);

  useHidePanelWebviewWhile(state.visible);

  useEffect(() => {
    if (!state.visible) return;
    const panel = panelRef.current;
    if (!panel) return;
    const cancelBtn = panel.querySelector<HTMLButtonElement>('[data-role="progress-cancel"]');
    (cancelBtn ?? panel).focus();
  }, [state.visible, state.cancel !== undefined]);

  if (!state.visible) return null;

  return (
    <Overlay
      open
      onClose={() => false}
      overlayClass="protected-surface"
      panelClass="surface surface-raised confirm-dialog progress-dialog protected-surface"
      panelRef={panelRef}
      panelProps={{ tabIndex: -1 }}
      panelRole="dialog"
      ariaModal
    >
      {progressDialogBody({ state })}
    </Overlay>
  );
}
