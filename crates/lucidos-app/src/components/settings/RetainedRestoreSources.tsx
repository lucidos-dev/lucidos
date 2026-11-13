/** Device-local journal review, separate from automatic reconstruction. These
 * originals are not server projections: header refresh re-reads owner evidence
 * and durable content, and admission changes revoke actions immediately. */
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { readRetainedRestoreSources, copyRetainedRestoreSource } from '../../store/restoreRuntime';
import type { RestoreSourceReview } from '../../store/restoreSourceReview';
import { restoreAdmissionReady } from '../../utils/restoreAdmission';
import { sourceGeneration } from '../../utils/restoreSourceGeneration';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';
import { formatBytes } from '../../utils/formatBytes';
import { Disclosure } from '../shared/Disclosure';
import { SectionHeader } from '../shared/SectionHeader';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';
import './retained-restore-sources.css';

const LABELS: Record<RestoreSourceReview['kind'], string> = {
  composer: 'Composer', 'queued-send': 'Queued message', 'uncertain-send': 'Uncertain send', upload: 'Upload',
};

export function RetainedRestoreSources() {
  const ready = restoreAdmissionReady();
  const [list, setList] = useState<Loadable<RestoreSourceReview[]>>({ status: 'not-loaded' });
  const request = useRef(0);
  const showLoading = useDelayedLoading(list);
  const refresh = useCallback(async () => {
    const ticket = ++request.current;
    setList(previous => previous.status === 'loaded' ? previous : { status: 'loading' });
    try {
      const data = await readRetainedRestoreSources();
      if (request.current === ticket) setList({ status: 'loaded', data });
    } catch (error) {
      if (request.current === ticket) setList(toFailed(error));
    }
  }, []);
  useEffect(() => {
    if (ready) void refresh();
    else setList({ status: 'not-loaded' });
    return () => { request.current++; };
  }, [ready, refresh]);
  usePanelRefresh('preserved restore sources', ready ? refresh : null);
  return <div class="settings-section retained-restore-sources">
    <h2>Preserved on this device <Explainer title="Preserved on this device">
      <p>Originals stay available after a restore. Copy into a draft to review before sending.</p>
      <p>Delivery of queued and uncertain messages is unknown. Previous context and selections are for review; they grant no access or approvals.</p>
    </Explainer></h2>
    {!ready ? <div class="empty-state">Reconnect to review preserved sources.</div> : list.status === 'failed'
      ? <LoadableError noun="preserved sources" error={list.error} onRetry={() => void refresh()} />
      : <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf count={2} row={() => <SourceRow />} />}>
        {list.status === 'loaded' && (list.data.length === 0
          ? <div class="empty-state">No preserved sources on this device.</div>
          : list.data.map(entry => <SourceRow key={JSON.stringify([entry.record.pageId, entry.record.generation, entry.source.threadId])}
            entry={entry} refresh={refresh} />))}
      </LoadingFade>}
  </div>;
}

function SourceRow({ entry, refresh }: { entry?: RestoreSourceReview; refresh?: () => Promise<void> }) {
  const [open, setOpen] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copyError, setCopyError] = useState<string>();
  const active = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const source = entry?.source;
  async function copy() {
    if (!entry || copying) return;
    setCopying(true);
    setCopyError(undefined);
    const generation = sourceGeneration();
    try {
      const destination = await copyRetainedRestoreSource({ pageId: entry.record.pageId,
        generation: entry.record.generation, sourceThreadId: entry.source.threadId });
      if (!active.current || !restoreAdmissionReady() || sourceGeneration() !== generation) return;
      focusThreadOrBootstrap(destination);
      await refresh?.();
    } catch (error) {
      if (active.current) setCopyError(errorDetail(error));
      else showToast(`Could not copy preserved source: ${errorDetail(error)}`, 'error');
    } finally { if (active.current) setCopying(false); }
  }
  return <div class="retained-restore-source">
    <SectionHeader title={<SkText>{entry ? LABELS[entry.kind] : 'Composer'}</SkText>}
      collapsed={!open} onToggle={() => setOpen(value => !value)} />
    {entry && <div class="list-row"><div class="list-row-info">
      <div class="list-row-details list-row-details-prose">
        {entry.ownership === 'live' || entry.ownership === 'unknown' ? 'Another tab may still be editing' : 'Preserved original'}
        {entry.delivery === 'unknown' && ' · Delivery unknown'}
      </div>
      {!open && <div class="list-row-details list-row-details-prose retained-restore-summary">{source!.draft.text || 'No message text'}</div>}
    </div></div>}
    <Disclosure open={open}>
      {entry && source && <div class="retained-restore-body">
        <strong>Message</strong><pre>{source.draft.text || 'No message text'}</pre>
        {source.uploads?.map(upload => <p key={upload.localId}>{upload.name} · {upload.mime} · {formatBytes(upload.size)}</p>)}
        {source.draft.image_hashes.length > 0 && <p>{source.draft.image_hashes.length} preserved image attachment(s)</p>}
        <strong>Context and previous selections</strong>
        <pre>{JSON.stringify({ mode: source.draft.mode, selection: source.selection,
          pendingSelection: entry.pendingSelection, request: source.unsentMessage?.body,
          queuedIntent: source.queuedUploadSend?.intent }, null, 2)}</pre>
        {entry.related.length > 0 && <p>Shares attachment content with {entry.related.length} other preserved source(s).</p>}
        {copyError && <p class="error-text">Could not copy preserved source: {copyError}</p>}
        <div class="retained-restore-actions"><button type="button" class="action-btn" disabled={copying || !restoreAdmissionReady()}
          onClick={() => void copy()}>{copying ? 'Copying…' : 'Copy to review draft'}</button></div>
      </div>}
    </Disclosure>
  </div>;
}
