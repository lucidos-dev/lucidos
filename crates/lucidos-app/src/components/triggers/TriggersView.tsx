import { triggers } from '../../store/store';
import { openAddTrigger } from '../../store/actions/triggers';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { hasNoMoreRuns } from '../../store/types';
import { TriggerItem } from './TriggerItem';

export function TriggersView() {
  const loadable = triggers.value;
  const showLoading = useDelayedLoading(loadable);

  if (loadable.status === 'failed') {
    return (
      <div class="content-view active">
        <div class="list-rows">
          <div class="empty-state error-text">
            Failed to load triggers: {loadable.error}
          </div>
        </div>
      </div>
    );
  }

  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="content-view active">
        <div class="list-rows">
          <div class="loading-spinner" />
        </div>
      </div>
    );
  }

  const sorted = [...loadable.data].sort((a, b) => {
    const aNoMore = hasNoMoreRuns(a);
    const bNoMore = hasNoMoreRuns(b);
    if (aNoMore !== bNoMore) return aNoMore ? 1 : -1;
    return 0;
  });

  return (
    <div class="content-view active">
      <div class="list-rows">
        {sorted.map((task) => (
          <TriggerItem key={task.id} task={task} />
        ))}
        <div class="list-row-add-card" onClick={openAddTrigger}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">Add Trigger</div>
        </div>
      </div>
    </div>
  );
}
