import type { TriggerInfo } from '../../store/types';
import { deriveTriggerType, hasNoMoreRuns } from '../../store/types';
import {
  openEditTrigger,
  toggleTrigger,
  deleteTrigger,
} from '../../store/actions/triggers';
import { formatShortDate, formatShortTime } from '../../utils/formatTime';
import { describeCron } from '../../utils/describeCron';

interface Props {
  trigger: TriggerInfo;
}

export function TriggerItem({ trigger }: Props) {
  const lastRunDate = trigger.last_run ? new Date(trigger.last_run) : null;
  const lastRunStr = lastRunDate ? `${formatShortDate(lastRunDate)} ${formatShortTime(lastRunDate)}` : null;
  const triggerType = deriveTriggerType(trigger);
  const noMoreRuns = hasNoMoreRuns(trigger);

  return (
    <div class={`list-row trigger-row clickable${trigger.paused ? ' trigger-disabled' : ''}`} onClick={() => openEditTrigger(trigger.id)}>
      <div class="list-row-info">
        <div class="title list-row-name">{trigger.name}</div>
        <div class="list-row-details">
          {noMoreRuns ? (
            <span class="trigger-no-more-runs">No more runs</span>
          ) : (
            <span class={trigger.paused ? 'trigger-paused' : 'trigger-enabled'}>
              {trigger.paused ? 'Paused' : 'Active'}
            </span>
          )}
          <span class={`label trigger-type-${triggerType}`}>
            {triggerType === 'hybrid' ? 'Hybrid' : triggerType === 'event' ? 'Event' : 'Schedule'}
          </span>
          <span class="label">
            {trigger.run.type === 'script' ? 'script' : 'LLM'}
          </span>
          {trigger.plugin_id && (
            <span class="label trigger-plugin-chip" data-tooltip={`Installed by the "${trigger.plugin_id}" plugin`}>
              from {trigger.plugin_id}
            </span>
          )}
        </div>
        {trigger.cron_expressions.length > 0 && (
          <ul class="trigger-cron-list">
            {trigger.cron_expressions.map((expr, i) => (
              <li key={i} class="trigger-cron-item">
                <span class="trigger-cron-desc">{describeCron(expr)}</span>
                <code class="trigger-cron">{expr}</code>
              </li>
            ))}
          </ul>
        )}
        {trigger.on && trigger.on.length > 0 && (
          <ul class="trigger-event-list">
            {trigger.on.map((sub, i) => (
              <li key={i} class="trigger-event-info">
                <span class="trigger-event-type">on {sub.event_type}</span>
                {sub.condition && (
                  <code class="trigger-condition">{JSON.stringify(sub.condition)}</code>
                )}
              </li>
            ))}
          </ul>
        )}
        {lastRunStr && (
          <div class="list-row-date">Last run {lastRunStr}</div>
        )}
      </div>
      <div class="list-row-actions">
        <button
          class="action-btn action-btn-danger"
          onClick={(e) => { e.stopPropagation(); void deleteTrigger(trigger.id, trigger.name); }}
        >
          Delete
        </button>
        <button
          class="action-btn"
          onClick={(e) => { e.stopPropagation(); void toggleTrigger(trigger.id, !trigger.paused); }}
        >
          {trigger.paused ? 'Resume' : 'Pause'}
        </button>
      </div>
    </div>
  );
}
