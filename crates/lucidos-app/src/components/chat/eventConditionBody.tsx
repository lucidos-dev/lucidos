import type { EventCondition } from '../../store/store';
import { plainEventName } from '../../store/thread-events/event-waits';

export function eventConditionTitle(open: EventCondition): string {
  return open.conditions.length > 1 ? 'Conditions' : 'Condition';
}

export function eventConditionBody(open: EventCondition) {
  const typeTooltip = plainEventName(open.eventType);
  return (
    <>
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
