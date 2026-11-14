import { useEffect, useState } from 'preact/hooks';
import { getEngineBuildLog } from '../../api/client';
import { engineBuildLogOpen, engineBuildLogVersion } from '../../store/engineBuildLog';
import type { Loadable } from '../../store/types';
import { loadingIfFresh, toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePaneCentre } from '../../hooks/usePaneCentre';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { SkeletonProvider, SkText } from '../shared/Skeleton';

/** Build output stays in the paired window, including Tauri and iOS PWAs. */
export function EngineBuildLogModal() {
  const [output, setOutput] = useState<Loadable<string>>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const showLoading = useDelayedLoading(output);
  const paneCentre = usePaneCentre('conversation');
  useVersionedRefresh(engineBuildLogVersion.value, false, () => setAttempt(value => value + 1));
  useEffect(() => {
    const controller = new AbortController();
    setOutput(loadingIfFresh);
    getEngineBuildLog(controller.signal)
      .then(data => { if (!controller.signal.aborted) setOutput({ status: 'loaded', data }); })
      .catch(error => { if (!controller.signal.aborted) setOutput(toFailed(error)); });
    return () => controller.abort();
  }, [attempt]);
  const close = () => { engineBuildLogOpen.value = false; };
  return (
    <Overlay open onClose={close} overlayClass="step-detail-overlay"
      panelClass="surface surface-raised surface-pane-centred step-detail-modal"
      panelStyle={paneCentre} panelRole="dialog" ariaModal dataRole="engine-build-log">
      <SurfaceHead title="Build log" onClose={close} closeLabel="Close build log" />
      <div class="surface-body step-detail-body" tabIndex={-1}>
        {output.status === 'failed'
          ? <LoadableError error={output.error} noun="build output" onRetry={() => setAttempt(value => value + 1)} />
          : <LoadingFade showSkeleton={showLoading} skeleton={<SkeletonProvider><pre class="step-detail-full"><SkText w="80%" /></pre></SkeletonProvider>}>
            {output.status === 'loaded' && <pre class="step-detail-full">{output.data || 'No build output captured yet.'}</pre>}
          </LoadingFade>}
      </div>
    </Overlay>
  );
}
