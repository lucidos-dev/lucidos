import { useEffect, useState } from 'preact/hooks';
import { getCommandCheckpointDiff } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { checkpointDiffModal } from '../../store/store';
import type { DiffFile } from '../../store/store';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { InlineDiffList } from '../files/RepoFilesView';
import { DiffSkeleton } from '../files/DiffView';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';

function close() {
  checkpointDiffModal.value = null;
}

type CheckpointDiff = { files: DiffFile[]; reclaimed: boolean };

export function CheckpointDiffModal() {
  const checkpoint = checkpointDiffModal.value;
  const checkpointId = checkpoint?.checkpoint_id;
  const [loadable, setLoadable] = useState<Loadable<CheckpointDiff>>({ status: 'loading' });
  const showLoading = useDelayedLoading(loadable);
  const paneCentre = usePaneCentre('conversation');

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
      panelClass="surface surface-raised surface-pane-centred step-detail-modal checkpoint-diff-modal"
      panelStyle={paneCentre}
      panelRole="dialog"
      ariaModal
      dataRole="checkpoint-diff-modal"
    >
      <SurfaceHead title="Checkpoint" onClose={close} closeLabel="Close checkpoint" />
      <div class="surface-body step-detail-body" tabIndex={-1}>
        <div class="step-detail-description">{checkpoint.summary}</div>
        <code class="step-detail-full">{checkpoint.command}</code>
        <div class="step-detail-section-label">What this step changed</div>
        {loadable.status === 'failed' && <LoadableError error={loadable.error} noun="the diff" />}
        {loadable.status !== 'failed' && (
          <LoadingFade showSkeleton={showLoading} skeleton={<DiffSkeleton files={2} class="folder-tree" />}>
            {loadable.status === 'loaded' && (
              loadable.data.reclaimed
                ? (
                  <div class="empty-state">
                    {'The snapshots behind this step have been reclaimed, so its changes can no longer be shown.'}
                  </div>
                )
                : <InlineDiffList files={loadable.data.files} />
            )}
          </LoadingFade>
        )}
      </div>
    </Overlay>
  );
}
