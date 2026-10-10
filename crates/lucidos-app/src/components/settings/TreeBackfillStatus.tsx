import type { ComponentChildren } from 'preact';
import {
  NO_BACKFILL_PROGRESS,
  backfillDetail,
  backfillPercent,
  treeBackfill,
} from '../../store/actions/treeBackfill';

const RETRYING = 'A summary failed, retrying.';

/** The Tree backfill's progress bar, shown while Tree is chosen. Structure
 *  first: the row and the bar's track draw at once, and only the values wait
 *  on the read, which the Memory section starts. The store signal is the
 *  subscription, so SSE frames move it. */
export function TreeBackfillStatus({ onPickModel }: { onPickModel: () => void }) {
  const loadable = treeBackfill.value;
  const state = loadable.status === 'loaded' ? loadable.data : null;
  // A Tree workspace still reads `off` until the switch's first frame lands.
  const progress = state?.state === 'running' ? state.progress : state?.state === 'off' ? NO_BACKFILL_PROGRESS : null;
  const ready = state?.state === 'ready';
  // Turns already read the trees; the older threads are still being built.
  const filling = state?.state === 'ready' && state.filling && state.filling.done < state.filling.total
    ? state.filling
    : null;
  const counted = filling ?? progress;
  const percent = filling ? backfillPercent(filling) : ready ? 100 : progress ? backfillPercent(progress) : 0;
  const working = counted !== null && counted.total > 0 && !counted.waiting_for_model;

  let label: ComponentChildren = null;
  if (loadable.status === 'failed') {
    label = <span class="error-text">Could not read the progress: {loadable.error}</span>;
  } else if (filling?.waiting_for_model) {
    label = (
      <>
        Ready. Older threads wait for a background model.{' '}
        <button type="button" class="accent-link" onClick={onPickModel}>Pick one</button>
      </>
    );
  } else if (filling) {
    label = `Ready. Filling in older threads, ${percent}%${filling.retrying ? `. ${RETRYING}` : ''}`;
  } else if (ready) {
    label = 'Ready';
  } else if (progress?.waiting_for_model) {
    label = (
      <>
        Waiting for a background model.{' '}
        <button type="button" class="accent-link" onClick={onPickModel}>Pick one</button>
      </>
    );
  } else if (progress && progress.total === 0) {
    label = 'Starting…';
  } else if (progress) {
    label = `Building memory, ${percent}%${progress.retrying ? `. ${RETRYING}` : ''}`;
  }

  return (
    <div
      class="settings-row memory-tree-status"
      data-role="tree-backfill"
      data-state={ready ? 'ready' : progress ? 'running' : loadable.status}
      data-working={working ? '' : undefined}
    >
      <span class="settings-row-label">Tree memory</span>
      <div class="memory-tree-progress">
        <div
          class="progress-bar"
          role="progressbar"
          aria-label="Tree memory"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
        >
          <div class="progress-bar-fill" style={{ width: `${percent}%` }} />
        </div>
        <span class="progress-label">{label}</span>
        {working && <span class="progress-label memory-tree-detail">{backfillDetail(counted)}</span>}
      </div>
    </div>
  );
}
