import { useCallback, useEffect, useRef, useState } from 'preact/hooks';
import { API, WORKSPACE_ID, parseWorkspaceRoute } from '../../utils/basePath';
import { json, mutatingFetch, requestGenerationGuard, throwIfNotOk } from '../../api/client/_core';
import { snapshotsVersion, showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { formatBytes } from '../../utils/formatBytes';
import { errorDetail } from '../../utils/errorDetail';
import { restoreFailureMessage } from '../../store/actions/restore-snapshot';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkText } from '../shared/Skeleton';

interface Snapshot {
  handle: string;
  kind: 'manual' | 'recovery';
  label: string | null;
  keep: boolean;
  workspace_id: string;
  workspace_name: string;
  created_at: string;
  components: { bytes: number }[];
}

export function SnapshotSection() {
  const [list, setList] = useState<Loadable<Snapshot[]>>({ status: 'not-loaded' });
  const [working, setWorking] = useState(false);
  const ticket = useRef(0);
  const showLoading = useDelayedLoading(list);
  const refresh = useCallback(async () => {
    const current = ++ticket.current;
    setList(previous => previous.status === 'loaded' ? previous : { status: 'loading' });
    try {
      const data = await json<Snapshot[]>(`${API}/snapshots`);
      if (current === ticket.current) setList({ status: 'loaded', data });
    } catch (error) { if (current === ticket.current) setList(toFailed(error)); }
  }, []);
  useEffect(() => { void refresh(); return () => { ticket.current++; }; }, [refresh]);
  usePanelRefresh('workspace snapshots', working ? null : refresh);
  useVersionedRefresh(snapshotsVersion.value, working, () => { void refresh(); });

  async function action(method: string, suffix: string, body?: unknown) {
    const response = await mutatingFetch(`${API}/snapshots${suffix}`, {
      method, headers: { 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    await throwIfNotOk(response);
  }
  async function perform(run: () => Promise<void>) {
    if (working) return;
    setWorking(true);
    try { await run(); }
    catch (error) { showToast(errorDetail(error), 'error', { title: 'Workspace snapshots' }); }
    finally { setWorking(false); }
  }
  async function restore(entry: Snapshot) {
    const checkGeneration = requestGenerationGuard();
    const workspace = WORKSPACE_ID ?? document.querySelector<HTMLMetaElement>('meta[name="lucidos-workspace-id"]')?.content;
    // No gateway identity means a bare engine, which the restore endpoint refuses.
    if (!workspace) throw new Error('Restoring needs the Lucidos gateway, and this engine runs without one');
    const target = parseWorkspaceRoute(workspace);
    if (!target || target.slug !== entry.workspace_id) throw new Error('Open this snapshot in its workspace before restoring');
    if (!await showConfirm(
      `Restore workspace "${entry.workspace_name}" from this snapshot? Current workspace data will be replaced and a recovery snapshot kept.\n\nSent messages, remote pushes and other external side effects are not undone. Current device access and pairing stay in place. Running work will stop and will not resume automatically.`,
      'Restore workspace', { title: 'Restore workspace', variant: 'danger' },
    )) return;
    checkGeneration();
    try {
      await action('POST', `/${encodeURIComponent(entry.handle)}/restore`,
        { expected_workspace_id: target.slug, confirmed: true });
      showToast('Restore requested', 'success', { title: 'Workspace snapshots' });
    } catch (error) {
      throw new Error(restoreFailureMessage(error));
    }
  }
  const entries = list.status === 'loaded' ? list.data : null;
  return <div class="settings-section">
    <h2>Workspace snapshots <Explainer title="Workspace snapshots">
      <p>Local copies of this workspace's database and files, on this machine. Snapshots need the existing secret keys and cannot replace an off-machine backup.</p>
      <p>Keep protects a snapshot from automatic retention. A recovery snapshot preserves the workspace before a restore.</p>
    </Explainer></h2>
    <div class="list-row"><div class="list-row-info">
      {entries && <><strong>{entries.length} snapshots</strong><div class="list-row-details list-row-details-prose">
        {formatBytes(entries.reduce((total, entry) => total + entry.components.reduce((sum, part) => sum + part.bytes, 0), 0))} on this machine
      </div></>}
    </div><div class="list-row-actions"><button class="action-btn" disabled={working}
      onClick={() => void perform(async () => {
        await action('POST', '', { keep: false });
        showToast('Snapshot capture started', 'success', { title: 'Workspace snapshots' });
      })}>Create snapshot</button></div></div>
    {list.status === 'failed' ? <LoadableError noun="workspace snapshots" error={list.error} onRetry={() => void refresh()} />
      : <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf count={2} row={() => <SnapshotRow />} />}>
        {entries && (entries.length === 0 ? <div class="empty-state">No workspace snapshots yet.</div>
          : entries.map(entry => <SnapshotRow key={entry.handle} entry={entry} disabled={working}
            onKeep={() => void perform(async () => { await action('PATCH', `/${encodeURIComponent(entry.handle)}`, { keep: !entry.keep }); await refresh(); })}
            onRestore={() => void perform(async () => { await restore(entry); })}
            onDelete={() => void perform(async () => {
              const checkGeneration = requestGenerationGuard();
              if (!await showConfirm('Delete this snapshot permanently?', 'Delete snapshot', { variant: 'danger' })) return;
              checkGeneration();
              await action('DELETE', `/${encodeURIComponent(entry.handle)}`);
              await refresh();
            })} />))}
      </LoadingFade>}
  </div>;
}

function SnapshotRow({ entry, disabled, onKeep, onRestore, onDelete }: {
  entry?: Snapshot; disabled?: boolean; onKeep?: () => void; onRestore?: () => void; onDelete?: () => void;
}) {
  return <div class="list-row"><div class="list-row-info">
    <strong><SkText>{entry ? entry.label || (entry.kind === 'recovery' ? 'Recovery snapshot' : 'Workspace snapshot') : 'Workspace snapshot'}</SkText></strong>
    <div class="list-row-details list-row-details-prose"><SkText>{entry
      ? `${new Date(entry.created_at).toLocaleString()} · ${formatBytes(entry.components.reduce((sum, part) => sum + part.bytes, 0))}${entry.keep ? ' · Kept' : ''}`
      : 'Date · Size'}</SkText></div>
  </div><div class="list-row-actions">
    <button class="action-btn action-btn-secondary" disabled={disabled} onClick={onKeep}>{entry?.keep ? 'Unkeep' : 'Keep'}</button>
    <button class="action-btn" disabled={disabled} onClick={onRestore}>Restore</button>
    <button class="action-btn action-btn-danger" disabled={disabled} onClick={onDelete}>Delete</button>
  </div></div>;
}
