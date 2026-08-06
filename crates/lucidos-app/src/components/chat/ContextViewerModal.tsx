import { contextViewer } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { ContextCapturePanel } from './ContextCapturePanel';
import { highlightEllipsis } from './highlightEllipsis';

function close() {
  contextViewer.value = null;
}

/** What the model was sent for one LLM call, opened from the context counter on
 *  the step row that call produced.
 *
 *  The counter is the only door: the step detail behind the rest of the row
 *  covers what the step DID (its command, its reasoning, its result), and this
 *  covers what the model was looking at when it decided to do it. Two questions,
 *  two views, so neither buries the other.
 *
 *  The step's description rides along as a subtitle, because the viewer is
 *  reached from a row and would otherwise open as a wall of sections with no
 *  statement of which call it belongs to. */
export function ContextViewerModal() {
  const open = contextViewer.value;
  if (!open) return null;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="step-detail-modal"
      panelRole="dialog"
      ariaModal
      dataRole="context-captured-modal"
    >
      <div class="step-detail-header">
        <span class="step-detail-status">Context</span>
      </div>
      {open.description && (
        <div class="step-detail-description">{highlightEllipsis(open.description)}</div>
      )}
      <ContextCapturePanel snap={open.snapshot} />
      <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
