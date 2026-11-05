import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  NO_BACKFILL_PROGRESS,
  backfillDetail,
  backfillPercent,
  continueTreeBackfill,
  treeBackfill,
} from '../../store/actions/treeBackfill';
import { formatBackfillSpend, usd } from './treeBackfillFormat';

const RETRYING = 'A summary failed, retrying.';

/** The Tree backfill's progress bar, shown while Tree is chosen. Structure
 *  first: the row and the bar's track draw at once, and only the values wait
 *  on the read, which the Memory section starts. The store signal is the
 *  subscription, so SSE frames move it. A backfill held at its spending cap
 *  offers Continue and a switch back to Classic. */
export function TreeBackfillStatus({ onPickModel, onSwitchToClassic }: {
  onPickModel: () => void;
  onSwitchToClassic: () => void;
}) {
  // The cap Continue was pressed at. The press stays spent until a frame
  // brings a new cap, so a second click cannot raise it twice.
  const [continuedFrom, setContinuedFrom] = useState<number | null>(null);
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
  const held = counted?.held && counted.cap_usd !== null ? { cap: counted.cap_usd, next: counted.next_cap_usd } : null;
  const working = counted !== null && counted.total > 0 && !counted.waiting_for_model && !held;
  const spend = counted ? formatBackfillSpend(counted) : null;

  let label: ComponentChildren = null;
  if (loadable.status === 'failed') {
    label = <span class="error-text">Could not read the progress: {loadable.error}</span>;
  } else if (held) {
    label = (
      <>
        Paused at the {usd(held.cap)} spending cap.{' '}
        {held.next !== null && (
          <>
            <button
              type="button"
              class="accent-link"
              disabled={continuedFrom === held.cap}
              onClick={() => {
                setContinuedFrom(held.cap);
                void continueTreeBackfill(held.cap).then((took) => {
                  if (!took) setContinuedFrom(null);
                });
              }}
            >
              Continue to {usd(held.next)}
            </button>{' '}
            or{' '}
          </>
        )}
        <button type="button" class="accent-link" onClick={onSwitchToClassic}>Switch to Classic</button>
      </>
    );
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
      data-state={held ? 'held' : ready ? 'ready' : progress ? 'running' : loadable.status}
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
        {spend && <span class="progress-label memory-tree-detail" data-role="tree-backfill-spend">{spend}</span>}
      </div>
    </div>
  );
}
