import type { TriggerInfo } from '../../store/types';
import { deriveTriggerType, hasNoMoreRuns } from '../../store/types';
import {
  openEditTrigger,
  toggleTrigger,
  deleteTrigger,
  runTriggerNow,
} from '../../store/actions/triggers';
import { formatShortDate, formatShortTime } from '../../utils/formatTime';
import { describeCron } from '../../utils/describeCron';
import { useSkeleton, SkText, SkBlock } from '../shared/Skeleton';

interface Props {
  trigger: TriggerInfo;
}

/** Self-skeletonizing trigger row: rendered with no trigger inside a
 *  SkeletonProvider (`<TriggerItem />`) it draws itself as a loading placeholder
 *  via the Sk* leaves (title + status/type/run chips + a last-run line); with a
 *  real trigger it renders normally. The prop is optional only to support the
 *  skeleton call; real call sites pass it. */
export function TriggerItem({ trigger }: Partial<Props>) {
  const sk = useSkeleton();
  const lastRunDate = trigger?.last_run ? new Date(trigger.last_run) : null;
  const lastRunStr = lastRunDate ? `${formatShortDate(lastRunDate)} ${formatShortTime(lastRunDate)}` : null;
  const triggerType = trigger ? deriveTriggerType(trigger) : 'schedule';
  const noMoreRuns = trigger ? hasNoMoreRuns(trigger) : false;
  // Mirrors the two server-side refusals, so the row never offers a button
  // that is guaranteed to error: an off-schedule run needs a cron schedule (an
  // event-only trigger has to have its event emitted instead) and an unpaused
  // trigger. Labelled "Run once", not "Run now": the Thread Queue panel's Run
  // now force-admits an already-queued entry, a different operation.
  const canRunOnce = !!trigger && !trigger.paused && triggerType !== 'event';

  return (
    <div
      class={`list-row trigger-row${sk ? '' : ' clickable'}${trigger?.paused ? ' trigger-disabled' : ''}`}
      onClick={sk ? undefined : () => { if (trigger) openEditTrigger(trigger.id); }}
    >
      <div class="list-row-info">
        <SkText class="title list-row-name" as="div" w="9rem">{trigger?.name}</SkText>
        <div class="list-row-details">
          <SkBlock w="3rem" h="1.25rem" round>
            {noMoreRuns ? (
              <span class="trigger-no-more-runs">No more runs</span>
            ) : (
              <span class={trigger?.paused ? 'trigger-paused' : 'trigger-enabled'}>
                {trigger?.paused ? 'Paused' : 'Active'}
              </span>
            )}
          </SkBlock>
          <SkBlock w="4rem" h="1.25rem" round>
            <span class={`label trigger-type-${triggerType}`}>
              {triggerType === 'hybrid' ? 'Hybrid' : triggerType === 'event' ? 'Event' : 'Schedule'}
            </span>
          </SkBlock>
          <SkBlock w="2.5rem" h="1.25rem" round>
            <span class="label">
              {trigger?.run.type === 'script' ? 'script' : 'LLM'}
            </span>
          </SkBlock>
          {trigger?.plugin_id && (
            <span class="label trigger-plugin-chip" data-tooltip={`Installed by the "${trigger.plugin_id}" plugin`}>
              from {trigger.plugin_id}
            </span>
          )}
        </div>
        {trigger && trigger.cron_expressions.length > 0 && (
          <ul class="trigger-cron-list">
            {trigger.cron_expressions.map((expr, i) => (
              <li key={i} class="trigger-cron-item">
                <span class="trigger-cron-desc">{describeCron(expr)}</span>
                <code class="trigger-cron">{expr}</code>
              </li>
            ))}
          </ul>
        )}
        {trigger?.on && trigger.on.length > 0 && (
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
        {(sk || lastRunStr) && (
          <div class="list-row-date trigger-last-run">
            <SkText as="span" w="8rem">{lastRunStr && `Last run ${lastRunStr}`}</SkText>
            {trigger?.last_run_status && (
              <span class={`trigger-run-status trigger-run-status-${trigger.last_run_status}`}>
                {trigger.last_run_status === 'ok' ? 'OK' : 'Failed'}
              </span>
            )}
          </div>
        )}
      </div>
      <div class="list-row-actions">
        <SkBlock w="3.75rem" h="2rem" round>
          <button
            class="action-btn action-btn-danger"
            onClick={(e) => { e.stopPropagation(); if (trigger) void deleteTrigger(trigger.id, trigger.name); }}
          >
            Delete
          </button>
        </SkBlock>
        <SkBlock w="3.75rem" h="2rem" round>
          <button
            class="action-btn"
            onClick={(e) => { e.stopPropagation(); if (trigger) void toggleTrigger(trigger.id, !trigger.paused); }}
          >
            {trigger?.paused ? 'Resume' : 'Pause'}
          </button>
        </SkBlock>
        {(sk || canRunOnce) && (
          <SkBlock w="4.5rem" h="2rem" round>
            <button
              class="action-btn"
              data-tooltip="Fire this trigger once, right now, outside its schedule"
              onClick={(e) => { e.stopPropagation(); if (trigger) void runTriggerNow(trigger.id); }}
            >
              Run once
            </button>
          </SkBlock>
        )}
      </div>
    </div>
  );
}
