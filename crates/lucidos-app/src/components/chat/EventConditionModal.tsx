import { eventConditionModal } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { plainEventName } from '../../store/thread-events/event-waits';

function close() {
  eventConditionModal.value = null;
}

/** The `condition` on one *event subscription*, opened from either PRESSABLE
 *  place saying "matching only": the transcript row's chip, and the waiting
 *  indicator's subscription line. Both ask `eventConditionDoor`, so neither can
 *  open something different or call it something different.
 *
 *  Those doors exist because the note they carry states that a filter is in play
 *  and nothing about what the filter says. That was deliberate: the raw operator
 *  JSON is developer-facing and does not belong on a line read by whoever is
 *  waiting. It belongs one tap away, which is here.
 *
 *  Each condition renders as a `type:` / `payload:` block: the event it waits
 *  for, and what that event's payload must match. The type is the RAW event
 *  name, and its plain name moves to the tooltip. */
export function EventConditionModal() {
  const open = eventConditionModal.value;
  if (!open) return null;
  const several = open.conditions.length > 1;
  const typeTooltip = plainEventName(open.eventType);

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
        <span class="step-detail-status">{several ? 'Conditions' : 'Condition'}</span>
      </div>
      {/* Pretty JSON, since a one-line dump is unreadable for the nested
          shapes a real filter takes (`$or` over several field paths). */}
      {open.conditions.map((condition, i) => (
        <pre key={i} class="step-detail-full" data-role="event-condition-json">
          <span class="event-condition-key">type:</span>{' '}
          <span data-role="event-condition-type" data-tooltip={typeTooltip}>{open.eventType}</span>
          {'\n'}
          <span class="event-condition-key">payload:</span>{' '}
          {JSON.stringify(condition, null, 2)}
        </pre>
      ))}
      <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
