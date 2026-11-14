import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { getTreeBackfillEstimate } from '../../api/client';
import type { EffortCost, TreeBackfillEstimate } from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { clampEffortFor, LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import { saveModelSelection, storedBackgroundSelection } from '../../store/actions/preferences';
import { displayModelName } from '../../store/thread-events/exchange';
import type { Loadable } from '../../store/types';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
import { BackgroundModelRow } from './BackgroundModelRow';
import type { BackgroundRows } from './useBackgroundModels';
import { formatCount, formatDuration, formatUsdCentral, usd } from './treeBackfillFormat';

/** The Tree estimate, priced for the compactor model and tier the workspace
 *  would run. Both surfaces that start Tree read it: the Start Tree dialog
 *  and the memory choice. */
export interface TreeEstimateRead {
  loadable: Loadable<TreeBackfillEstimate>;
  /** The delay-gated loading flag for the estimate itself. */
  showLoading: boolean;
  retry: () => void;
  estimate: TreeBackfillEstimate | null;
  model: string;
  effort: string | null;
  /** The estimate, once the model it is for is known too. */
  priced: TreeBackfillEstimate | null;
  /** The chosen model's row at the chosen tier, `null` while unpriced. */
  cost: EffortCost | null;
  /** A cost waits on both reads: the estimate, and which model it is for. */
  costLoading: boolean;
  /** Set once both reads landed and the model has no price. */
  unpriced: string | null;
}

export function useTreeEstimate(background: BackgroundRows): TreeEstimateRead {
  const [attempt, setAttempt] = useState(0);
  const { loadable, showLoading } = useLoadableFetch(getTreeBackfillEstimate, [attempt]);
  const resolved = background.row('model_summary_compaction');
  const stored = storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction');
  const model = stored.model ?? resolved?.model ?? '';
  const effort = stored.effort ?? resolved?.effort ?? null;
  const estimate = loadable.status === 'loaded' ? loadable.data : null;
  const modelKnown = model !== '';
  const priced = modelKnown ? estimate : null;
  const cost = priced?.costs.find((c) => c.model === model)?.by_effort.find((e) => e.effort === effort) ?? null;
  return {
    loadable,
    showLoading,
    retry: () => setAttempt((n) => n + 1),
    estimate,
    model,
    effort,
    priced,
    cost,
    costLoading: showLoading || (!modelKnown && background.error === null),
    unpriced: priced && !cost ? 'No estimate for this model' : null,
  };
}

/** One estimate figure. Its label draws at once; the value waits on the read,
 *  behind a delay-gated shimmer. */
export function EstimateItem({ label, value, loading }: {
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

/** The tier the engine recommends for `model`, while `effort` runs at another
 *  one, else `null`. A tier the model cannot run counts as the one it snaps to,
 *  which is what the picker shows. */
function offRecommendedTier(
  background: BackgroundRows,
  model: string,
  effort: string | null,
): string | null {
  const recommended = background
    .row('model_summary_compaction')
    ?.recommended.find((r) => r.model === model)?.effort ?? null;
  const runsAt = effort === null ? null : clampEffortFor(effort, model);
  return recommended !== null && recommended !== runsAt ? recommended : null;
}

/** Names the recommended tier while another is picked, and switches to it. */
function UseRecommendedTier({ model, tier }: { model: string; tier: string }) {
  const label = LUCIDOS_TIER_VOCABULARY.find((t) => t.value === tier)?.label ?? tier;
  return (
    <div class="settings-row-note" data-role="tree-recommended-tier">
      {label} is recommended for {displayModelName(model)}.{' '}
      <button
        type="button"
        class="accent-link"
        onClick={() => void saveModelSelection(
          'model_summary_compaction',
          'reasoning_summary_compaction',
          { model, reasoningEffort: tier },
        )}
      >
        Use {label}
      </button>
    </div>
  );
}

const COMPACTOR_MODEL = 'Compactor model';

/** The compactor model row, and the recommended tier while another is picked.
 *  A `label` heads a panel the picker sits in, and the picker keeps its name. */
export function CompactorModelRow({ background, read, label = COMPACTOR_MODEL }: {
  background: BackgroundRows;
  read: TreeEstimateRead;
  label?: string;
}) {
  const recommendedTier = offRecommendedTier(background, read.model, read.effort);
  return (
    <>
      <BackgroundModelRow
        label={label}
        fieldLabel={COMPACTOR_MODEL}
        modelKey="model_summary_compaction"
        reasoningKey="reasoning_summary_compaction"
        background={background}
      />
      {recommendedTier && <UseRecommendedTier model={read.model} tier={recommendedTier} />}
    </>
  );
}

/** Why the estimate could not be read, with a retry. */
export function EstimateFailed({ read }: { read: TreeEstimateRead }) {
  if (read.loadable.status !== 'failed') return null;
  return (
    <div class="settings-row-note error-text">
      Could not estimate the cost: {read.loadable.error}{' '}
      <button type="button" class="accent-link" onClick={read.retry}>Try again</button>
    </div>
  );
}

/** What the one-time summary of this workspace costs and how long it takes,
 *  then what a day costs after it. */
export function BackfillFigures({ read }: { read: TreeEstimateRead }) {
  const { estimate, priced, cost, unpriced, showLoading, costLoading } = read;
  // The chosen tier's own call time, when the estimate prices it.
  const times = cost ?? estimate;
  return (
    <dl class="memory-tree-estimate" data-role="tree-estimate" data-state={read.loadable.status}>
      <EstimateItem label="Calls" value={estimate && formatCount(estimate.calls)} loading={showLoading} />
      <EstimateItem
        label="Cost"
        value={priced && (cost ? formatUsdCentral(cost.backfill_usd_central, cost.backfill_usd) : unpriced)}
        loading={costLoading}
      />
      <EstimateItem
        label="Pauses at"
        value={priced && (cost ? usd(cost.cap_usd) : 'No cap for this model')}
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
  );
}
