import { useState } from 'preact/hooks';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { BackfillFigures, CompactorModelRow, EstimateFailed, useTreeEstimate } from './TreeEstimate';
import type { BackgroundRows } from './useBackgroundModels';

const TITLE = 'Start Tree memory';

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
  const [starting, setStarting] = useState(false);
  const read = useTreeEstimate(background);

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
        <CompactorModelRow background={background} read={read} />
        {read.loadable.status === 'failed' ? <EstimateFailed read={read} /> : <BackfillFigures read={read} />}
        <div class="settings-row-note">
          An estimate from this workspace's history. Entry sizes are approximate,
          so each figure is a range. Nothing is spent until you start. Building
          pauses at the cap and asks before spending more. Turns move to Tree once
          the workspace and the last week's threads are summarized, and older
          threads fill in after.
        </div>
      </div>
      <div class="surface-foot">
        <button type="button" class="action-btn action-btn-secondary" onClick={onCancel}>Cancel</button>
        <button
          type="button"
          class="action-btn action-btn-confirm"
          disabled={!read.estimate || starting}
          onClick={() => void start()}
        >
          Start Tree
        </button>
      </div>
    </Overlay>
  );
}
