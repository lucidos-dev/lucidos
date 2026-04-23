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
  task: TriggerInfo;
}

export function TriggerItem({ task }: Props) {
  const lastRunDate = task.last_run ? new Date(task.last_run) : null;
  const lastRunStr = lastRunDate ? `${formatShortDate(lastRunDate)} ${formatShortTime(lastRunDate)}` : null;
  const triggerType = deriveTriggerType(task);
  const noMoreRuns = hasNoMoreRuns(task);

  return (
    <div class={`list-row trigger-row${task.enabled ? '' : ' trigger-disabled'}`}>
      <div class="list-row-info">
        <div class="title list-row-name">{task.name}</div>
        <div class="list-row-details">
          {noMoreRuns ? (
            <span class="trigger-no-more-runs">No more runs</span>
          ) : (
            <span class={task.enabled ? 'trigger-enabled' : 'trigger-paused'}>
              {task.enabled ? 'Active' : 'Paused'}
            </span>
          )}
          <span class={`label trigger-type-${triggerType}`}>
            {triggerType === 'hybrid' ? 'Hybrid' : triggerType === 'event' ? 'Event' : 'Schedule'}
          </span>
          <span class="label">
            {task.run.type === 'script' ? 'script' : 'LLM'}
          </span>
        </div>
        {task.cron_expressions.length > 0 && (
          <ul class="trigger-cron-list">
            {task.cron_expressions.map((expr, i) => (
              <li key={i} class="trigger-cron-item">
                <span class="trigger-cron-desc">{describeCron(expr)}</span>
                <code class="trigger-cron">{expr}</code>
              </li>
            ))}
          </ul>
        )}
        {task.on && (
          <div class="trigger-event-info">
            <span class="trigger-event-type">on {task.on}</span>
            {task.condition && (
              <code class="trigger-condition">{JSON.stringify(task.condition)}</code>
            )}
          </div>
        )}
        {lastRunStr && (
          <div class="list-row-date">Last run {lastRunStr}</div>
        )}
      </div>
      <div class="list-row-actions">
        <button
          class="action-btn action-btn-danger"
          onClick={() => deleteTrigger(task.id, task.name)}
        >
          Delete
        </button>
        <button
          class="action-btn trigger-toggle-btn"
          onClick={() => toggleTrigger(task.id, !task.enabled)}
        >
          {task.enabled ? 'Pause' : 'Resume'}
        </button>
        <button
          class="action-btn"
          onClick={() => openEditTrigger(task.id)}
        >
          Edit
        </button>
      </div>
    </div>
  );
}
