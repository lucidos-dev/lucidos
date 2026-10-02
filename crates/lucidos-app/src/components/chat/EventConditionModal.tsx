import { eventConditionModal } from '../../store/store';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { eventConditionBody, eventConditionTitle } from './eventConditionBody';

function close() {
  eventConditionModal.value = null;
}

/** The `condition` on one *event subscription*, opened from the transcript
 *  row's "with a condition" chip through `eventConditionDoor`.
 *
 *  That door exists because the note it carries states that a filter is in
 *  play and nothing about what the filter says. That was deliberate: the raw
 *  operator JSON is developer-facing and does not belong on a line read by
 *  whoever is waiting. It belongs one tap away, which is here. The waiting
 *  panel asks the same door but drills in instead, since a popover never opens
 *  a second layer. */
export function EventConditionModal() {
  const open = eventConditionModal.value;
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
      dataRole="event-condition-modal"
      panelProps={{ 'aria-label': `Condition on ${open.eventType}` }}
    >
      <SurfaceHead title={eventConditionTitle(open)} onClose={close} closeLabel="Close condition" />
      <div class="surface-body step-detail-body" tabIndex={-1}>
        {eventConditionBody(open)}
      </div>
    </Overlay>
  );
}
