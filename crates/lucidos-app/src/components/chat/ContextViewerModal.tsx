import { contextViewer } from '../../store/store';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { ContextCapturePanel } from './ContextCapturePanel';
import { highlightEllipsis } from './highlightEllipsis';

function close() {
  contextViewer.value = null;
}

export function ContextViewerModal() {
  const open = contextViewer.value;
  const paneCentre = usePaneCentre('conversation');
  if (!open) return null;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="surface surface-raised surface-pane-centred step-detail-modal"
      panelStyle={paneCentre}
      panelRole="dialog"
      ariaModal
      dataRole="context-captured-modal"
    >
      <SurfaceHead title="Context" onClose={close} closeLabel="Close context" />
      <div class="surface-body step-detail-body" tabIndex={-1}>
        {open.description && (
          <div class="step-detail-description">{highlightEllipsis(open.description)}</div>
        )}
        <ContextCapturePanel snap={open.snapshot} />
      </div>
    </Overlay>
  );
}
