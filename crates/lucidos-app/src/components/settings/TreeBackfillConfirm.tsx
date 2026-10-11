import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { getTreeBackfillEstimate } from '../../api/client';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { clampEffortFor, LUCIDOS_TIER_VOCABULARY } from '../../store/actions/models';
import { saveModelSelection, storedBackgroundSelection } from '../../store/actions/preferences';
import { displayModelName } from '../../store/thread-events/exchange';
import { Overlay } from '../shared/Overlay';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
import { SurfaceHead } from '../shared/Surface';
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

const TITLE = 'Start Tree memory';

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

/** A modal: what choosing Tree would cost, with the compactor model to run it,
 *  and the only button that switches the workspace to Tree. Nothing is spent
 *  before a press of Start, and Start waits for the estimate. */
export function TreeBackfillConfirm({ background, anchor, onStart, onCancel }: {
  background: BackgroundRows;
  /** The Tree button, which takes focus back when the modal closes. */
  anchor: HTMLElement | null;
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
  const recommendedTier = offRecommendedTier(background, model, effort);
  const estimate = loadable.status === 'loaded' ? loadable.data : null;
  // A cost needs both reads: the estimate, and which model it is for.
  const modelKnown = model !== '';
  const priced = modelKnown ? estimate : null;
  const costLoading = showLoading || (!modelKnown && background.error === null);
  const modelCost = priced?.costs.find((c) => c.model === model) ?? null;
  const cost = modelCost?.by_effort.find((e) => e.effort === effort) ?? null;
  const unpriced = priced && !cost ? 'No estimate for this model' : null;
  // The chosen tier's own call time, when the estimate prices it.
  const times = cost ?? estimate;

  async function start() {
    setStarting(true);
    try {
      await onStart();
    } finally {
      setStarting(false);
    }
  }

  return (
    <Overlay
      open
      onClose={onCancel}
      anchor={anchor}
      panelClass="surface surface-raised memory-tree-confirm"
      panelRole="dialog"
      ariaModal
      dataRole="tree-confirm"
      panelProps={{ 'aria-label': TITLE }}
    >
      <SurfaceHead title={TITLE} onClose={onCancel} />
      <div class="surface-body memory-tree-confirm-body">
        <BackgroundModelRow
          label="Compactor model"
          modelKey="model_summary_compaction"
          reasoningKey="reasoning_summary_compaction"
          background={background}
        />
        {recommendedTier && <UseRecommendedTier model={model} tier={recommendedTier} />}
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
      </div>
      <div class="surface-foot">
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
    </Overlay>
  );
}
