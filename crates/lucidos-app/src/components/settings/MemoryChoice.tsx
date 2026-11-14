import { useState } from 'preact/hooks';
import { setMemoryModule } from '../../store/actions/preferences';
import { loadTreeBackfill } from '../../store/actions/treeBackfill';
import { Overlay } from '../shared/Overlay';
import { SkeletonProvider, SkText } from '../shared/Skeleton';
import { SurfaceHead } from '../shared/Surface';
import { BackfillFigures, CompactorModelRow, EstimateFailed, useTreeEstimate, type TreeEstimateRead } from './TreeEstimate';
import { formatUsd, usd } from './treeBackfillFormat';
import { monthlyCost, USAGE_PROFILES } from './usageProfiles';
import { useBackgroundModels } from './useBackgroundModels';

const TITLE = 'Choose how this workspace remembers';

/** The *memory choice*: a workspace that never chose its memory module picks
 *  one here. Every way out writes `memory_module`, so it asks once. */
export function MemoryChoice() {
  const background = useBackgroundModels();
  const read = useTreeEstimate(background);
  const [starting, setStarting] = useState(false);

  function chooseClassic() {
    void setMemoryModule('classic');
  }

  // The Start Tree press in Settings: the compactor starts the backfill under
  // the same cap.
  async function chooseTree() {
    setStarting(true);
    try {
      await setMemoryModule('tree');
      await loadTreeBackfill();
    } finally {
      setStarting(false);
    }
  }

  return (
    <Overlay
      open
      onClose={chooseClassic}
      panelClass="surface surface-raised memory-choice"
      panelRole="dialog"
      ariaModal
      dataRole="memory-choice"
      panelProps={{ 'aria-label': TITLE }}
    >
      <SurfaceHead title={TITLE} onClose={chooseClassic} closeLabel="Close, and stay on Classic" />
      <div class="surface-body memory-choice-body">
        <p class="memory-choice-lead">
          Each turn gets a memory of your earlier work. Pick how Lucidos builds
          it for this workspace.
        </p>
        <div class="memory-choice-cards">
          <section class="memory-choice-card">
            <div class="memory-choice-card-title">Classic <span class="label label-success">Almost free</span></div>
            <p>
              Keeps a running summary of each thread and recalls saved
              memories. On long threads it makes a small summary call now
              and then.
            </p>
            <button type="button" class="action-btn action-btn-secondary" onClick={chooseClassic}>
              Use Classic
            </button>
          </section>
          <section class="memory-choice-card">
            <div class="memory-choice-card-title">Tree <span class="label label-warning">Costs money</span></div>
            <p>
              Gives each turn a layered summary of the whole workspace and of
              the thread. The agent can open any line down to the exact message.
            </p>
            <p>
              A compactor model writes the summaries after each turn. So the
              cost grows with how much you use the workspace, and depends on
              the model.
            </p>
            <button
              type="button"
              class="action-btn action-btn-confirm"
              disabled={!read.estimate || starting}
              onClick={() => void chooseTree()}
            >
              Use Tree
            </button>
          </section>
        </div>
        <section class="memory-choice-cost">
          <CompactorModelRow background={background} read={read} label="What Tree costs a month" />
          {read.loadable.status === 'failed' ? <EstimateFailed read={read} /> : <MonthlyTiles read={read} />}
          {/* Only a workspace with history has a one-time summary to price. */}
          {read.estimate && read.estimate.calls.high > 0 && <BackfillFigures read={read} />}
        </section>
        <p class="memory-choice-note">
          If you close this, the workspace stays on Classic. You can change it
          any time under Settings &gt; System &gt; Memory.
        </p>
      </div>
    </Overlay>
  );
}

/** One tile per usage profile: a month of its turns at one turn's price. */
function MonthlyTiles({ read }: { read: TreeEstimateRead }) {
  const { cost, unpriced, costLoading } = read;
  return (
    <>
      <div class="memory-choice-tiles">
        {USAGE_PROFILES.map((profile) => {
          const month = cost && monthlyCost(profile, cost);
          return (
            <div key={profile.name} class="memory-choice-tile" data-role="memory-choice-tile">
              <div class="memory-choice-tile-name">{profile.name}</div>
              <div class="memory-choice-tile-usd">
                {month
                  ? <>{usd(month.central)} <small>a month</small></>
                  : unpriced
                    ? <small>{unpriced}</small>
                    : costLoading && <SkeletonProvider><SkText w="5rem" /></SkeletonProvider>}
              </div>
              <div class="memory-choice-tile-range">{month && formatUsd(month.range)}</div>
              <div class="memory-choice-tile-does">{profile.does}</div>
            </div>
          );
        })}
      </div>
      <div class="memory-choice-note">
        {cost && `About ${usd(cost.turn_usd_central)} a turn. Each figure is a range.`}
      </div>
    </>
  );
}
