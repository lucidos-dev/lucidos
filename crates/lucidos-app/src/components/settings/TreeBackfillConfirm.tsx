import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { getTreeBackfillEstimate } from '../../api/client';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { storedBackgroundSelection } from '../../store/actions/preferences';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
import { BackgroundModelRow } from './BackgroundModelRow';
import type { BackgroundRows } from './useBackgroundModels';
import { formatCount, formatDuration, formatUsdCentral } from './treeBackfillFormat';

/** One estimate figure. Its label draws at once; the value waits on the read,
 *  behind a delay-gated shimmer. */
function EstimateItem({ label, value, loading }: {
  label: string;
  value: ComponentChildren | null;
  loading: boolean;
}) {
  return (
    <div class="memory-tree-estimate-item">
      <dt>{label}</dt>
      <dd>
        {value ?? (loading ? <SkeletonProvider><SkText w="5rem" /></SkeletonProvider> : null)}
      </dd>
    </div>
  );
}

/** What choosing Tree would cost, with the compactor model to run it, and the
 *  only button that switches the workspace to Tree. Nothing is spent before a
 *  press of Start, and Start waits for the estimate. */
export function TreeBackfillConfirm({ background, onStart, onCancel }: {
  background: BackgroundRows;
  onStart: () => Promise<void>;
  onCancel: () => void;
}) {
  const [attempt, setAttempt] = useState(0);
  const [starting, setStarting] = useState(false);
  const { loadable, showLoading } = useLoadableFetch(getTreeBackfillEstimate, [attempt]);
  const resolved = background.row('model_summary_compaction');
  const stored = storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction');
  const model = stored.model ?? resolved?.model ?? '';
  const effort = stored.effort ?? resolved?.effort ?? null;
  const estimate = loadable.status === 'loaded' ? loadable.data : null;
  // A cost needs both reads: the estimate, and which model it is for.
  const modelKnown = model !== '';
  const priced = modelKnown ? estimate : null;
  const costLoading = showLoading || (!modelKnown && background.error === null);
  const modelCost = priced?.costs.find((c) => c.model === model) ?? null;
  const cost = modelCost?.by_effort.find((e) => e.effort === effort) ?? null;
  const unpriced = priced && !cost ? 'No estimate for this model' : null;
  // The chosen model's measured call time, when the estimate prices it.
  const times = modelCost ?? estimate;

  async function start() {
    setStarting(true);
    try {
      await onStart();
    } finally {
      setStarting(false);
    }
  }

  return (
    <div class="memory-tree-confirm" data-role="tree-confirm">
      <BackgroundModelRow
        label="Compactor model"
        nested
        modelKey="model_summary_compaction"
        reasoningKey="reasoning_summary_compaction"
        background={background}
      />
      {loadable.status === 'failed' ? (
        <div class="settings-row-note error-text">
          Could not estimate the cost: {loadable.error}{' '}
          <button type="button" class="accent-link" onClick={() => setAttempt((n) => n + 1)}>Try again</button>
        </div>
      ) : (
        <dl class="memory-tree-estimate" data-role="tree-estimate" data-state={loadable.status}>
          <EstimateItem label="Calls" value={estimate && formatCount(estimate.calls)} loading={showLoading} />
          <EstimateItem
            label="Cost"
            value={priced && (cost ? formatUsdCentral(cost.backfill_usd_central, cost.backfill_usd) : unpriced)}
            loading={costLoading}
          />
          <EstimateItem label="Usable in" value={times && formatDuration(times.usable_secs)} loading={showLoading} />
          <EstimateItem label="Complete in" value={times && formatDuration(times.complete_secs)} loading={showLoading} />
          <EstimateItem
            label="Then, a day"
            value={priced && (cost
              ? `${formatUsdCentral(cost.daily_usd_central, cost.daily_usd)}, ${formatCount(priced.daily_calls)} calls`
              : `${formatCount(priced.daily_calls)} calls`)}
            loading={costLoading}
          />
        </dl>
      )}
      <div class="settings-row-note">
        An estimate from this workspace's history. Entry sizes are approximate,
        so each figure is a range. Nothing is spent until you start. Turns move
        to Tree once the workspace and the last week's threads are summarized,
        and older threads fill in after.
      </div>
      <div class="memory-tree-confirm-actions">
        <button type="button" class="action-btn action-btn-secondary" onClick={onCancel}>Cancel</button>
        <button
          type="button"
          class="action-btn action-btn-confirm"
          disabled={!estimate || starting}
          onClick={() => void start()}
        >
          Start Tree
        </button>
      </div>
    </div>
  );
}
