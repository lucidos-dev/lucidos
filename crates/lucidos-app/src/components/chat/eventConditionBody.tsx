import type { EventCondition } from '../../store/store';
import { plainEventName } from '../../store/thread-events/event-waits';

/** "Condition" or "Conditions", the title both surfaces give the view. */
export function eventConditionTitle(open: EventCondition): string {
  return open.conditions.length > 1 ? 'Conditions' : 'Condition';
}

/** Each condition as a `type:` / `payload:` block: the event it waits for, and
 *  what that event's payload must match. The type is the RAW event name, and
 *  its plain name moves to the tooltip.
 *
 *  One body for both places a condition opens, so they cannot drift: the
 *  transcript chip's popover, and the waiting panel's drill-in. A plain function
 *  rather than a component, so a test can walk the tree without rendering it.
 *
 *  Keep these out of the popover's module. The popover is code-split, and a
 *  static import of it from the waiting panel folds it into the shell chunk. */
export function eventConditionBody(open: EventCondition) {
  const typeTooltip = plainEventName(open.eventType);
  return (
    <>
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
    </>
  );
}
