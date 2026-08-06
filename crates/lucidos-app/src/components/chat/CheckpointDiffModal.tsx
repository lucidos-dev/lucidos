import { useEffect, useState } from 'preact/hooks';
import { getCommandCheckpointDiff } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { checkpointDiffModal } from '../../store/store';
import type { DiffFile } from '../../store/store';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { InlineDiffList } from '../files/RepoFilesView';
import { LoadableError } from '../shared/LoadableError';
import { Overlay } from '../shared/Overlay';

function close() {
  checkpointDiffModal.value = null;
}

type CheckpointDiff = { files: DiffFile[]; reclaimed: boolean };

/** What one checkpointed command changed: the diff between the snapshot taken
 *  before it ran and the one taken after.
 *
 *  This is the answer to "what am I actually undoing". Before it existed the
 *  card offered a bare Undo, so the only way to learn what a destructive step
 *  had done was to click it and compare the workspace afterwards. The two
 *  snapshots the guard already takes are exactly the two sides of this diff, so
 *  it costs one `git diff` and no new bookkeeping.
 *
 *  Renders through `InlineDiffList`, the same component the repository and
 *  change diffs use, so a checkpoint diff looks like every other diff in the
 *  product rather than a second dialect. */
export function CheckpointDiffModal() {
  const checkpoint = checkpointDiffModal.value;
  const checkpointId = checkpoint?.checkpoint_id;
  const [loadable, setLoadable] = useState<Loadable<CheckpointDiff>>({ status: 'loading' });
  const showLoading = useDelayedLoading(loadable);

  // Keyed on the id rather than the event object: the card re-renders (and
  // hands us a fresh object) when the paired revert lands, and refetching the
  // same diff on that flip would blank the panel the user is reading.
  useEffect(() => {
    if (!checkpointId) return;
    let cancelled = false;
    setLoadable({ status: 'loading' });
    getCommandCheckpointDiff(checkpointId)
      .then(data => { if (!cancelled) setLoadable({ status: 'loaded', data }); })
      .catch(e => { if (!cancelled) setLoadable(toFailed(e)); });
    return () => { cancelled = true; };
  }, [checkpointId]);

  if (!checkpoint) return null;

  return (
    <Overlay
      open
      onClose={close}
      overlayClass="step-detail-overlay"
      panelClass="step-detail-modal checkpoint-diff-modal"
      panelRole="dialog"
      ariaModal
      dataRole="checkpoint-diff-modal"
    >
      <div class="step-detail-description">{checkpoint.summary}</div>
      <code class="step-detail-full">{checkpoint.command}</code>
      <div class="step-detail-section-label">What this step changed</div>
      {loadable.status === 'failed' && <LoadableError error={loadable.error} noun="the diff" />}
      {loadable.status === 'loading' && (showLoading ? <div class="loading-spinner" /> : null)}
      {loadable.status === 'loaded' && (
        // A reclaimed pair is not an empty diff, and must not read as one. The
        // snapshots behind an old card are dropped after 30 days, and cards
        // written before the post image existed never had a second side to
        // diff against.
        loadable.data.reclaimed
          ? (
            <div class="empty-state">
              {'The snapshots behind this step have been reclaimed, so its changes can no longer be shown.'}
            </div>
          )
          : <InlineDiffList files={loadable.data.files} />
      )}
      <button class="action-btn step-detail-close" onClick={close}>Close</button>
    </Overlay>
  );
}
