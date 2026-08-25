import { eventConditionModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { eventNameChip } from './EventRow';

function close() {
  eventConditionModal.value = null;
}

/** The `condition` on one *event subscription*, opened from either PRESSABLE
 *  place saying "(filtered)": the transcript row's chip, and the waiting
 *  indicator's subscription line. Both ask `eventConditionDoor`, so neither can
 *  open something different or call it something different.
 *
 *  Those doors exist because the note they carry states that a filter is in play
 *  and nothing about what the filter says. That was deliberate: the raw operator
 *  JSON is developer-facing and does not belong on a line read by whoever is
 *  waiting. It belongs one tap away, which is here.
 *
 *  The event type rides along as the subject, spelled with the same chip atom
 *  the row uses, so the modal states which subscription it opened. */
export function EventConditionModal() {
  const open = eventConditionModal.value;
  if (!open) return null;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="step-detail-modal"
      panelRole="dialog"
      ariaModal
      dataRole="event-condition-modal"
      panelProps={{ 'aria-label': `Condition on ${open.eventType}` }}
    >
      <div class="step-detail-header">
        <span class="step-detail-status">Condition</span>
      </div>
      <div class="step-detail-description">{eventNameChip({ kind: 'chip', name: open.eventType })}</div>
      {/* Says what the filter DOES, since a reader landing on a wall of
          operators needs to know it narrows rather than widens the watch. */}
      <div class="step-detail-note">
        Only an event whose payload satisfies this resumes the thread.
      </div>
      {/* Pretty JSON, which is how a condition is written and how the trigger
          editor already shows one. A one-line dump would be unreadable for the
          nested shapes a real filter takes (`$or` over several field paths). */}
      <pre class="step-detail-full" data-role="event-condition-json">
        {JSON.stringify(open.condition, null, 2)}
      </pre>
      <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
