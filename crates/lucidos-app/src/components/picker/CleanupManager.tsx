import { useSignal } from '@preact/signals';
import './CleanupManager.css';
import { useEffect, useRef } from 'preact/hooks';
import {
  getCleanupReview, listTrash, freezeCleanup, confirmCleanup, freezeEmptyTrash,
  confirmEmptyTrash, restoreTrash, abandonTrashStaging,
  type CleanupRow, type CleanupReview, type CleanupBatch, type EmptyTrashBatch,
  type TrashEntry, type CleanupResult,
} from '../../api/client/control';
import { toFailed, type Loadable } from '../../store/types';
import { formatBytes } from '../../utils/formatBytes';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { Disclosure } from '../shared/Disclosure';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkText, useSkeleton } from '../shared/Skeleton';
import { cleanupTotals, hasCleanupWork, isCleanupSuggested, sortCleanupRows, CLEANUP_NUDGE_BYTES, CLEANUP_NUDGE_INTERVAL_MS } from './cleanupReview';

function ReviewRow({ row, selected = false, disabled = false, onSelect, onReview }: {
  row?: CleanupRow; selected?: boolean; disabled?: boolean;
  onSelect?: () => void; onReview?: () => void;
}) {
  const skeleton = useSkeleton();
  if (skeleton || !row) return <li class="list-row"><SkText w="18em">Workspace</SkText></li>;
  const protectedRow = !row.workspace || ['user', 'unknown'].includes(row.workspace.origin);
  return <li class="list-row">
    {!protectedRow && <input type="checkbox" aria-label={`Select ${row.workspace?.name ?? row.directory}`}
      checked={selected} disabled={disabled || !row.fingerprint} onChange={onSelect} />}
    <div class="list-row-info">
      <strong>{row.workspace?.name ?? row.database?.name ?? row.directory}</strong>
      <p>{row.workspace?.origin ?? 'unknown'} · {row.state} · {row.running === true ? 'running' : row.running === false ? 'stopped' : 'runtime unknown'}</p>
      <p>Folder: {row.folder_bytes == null ? 'unknown' : formatBytes(row.folder_bytes)} · Database: {row.database ? formatBytes(row.database.bytes) : 'unknown'}</p>
      <p>Last folder activity: {row.folder_modified ?? 'unknown'} · Last event: {row.database?.last_event ?? 'unknown'} · Last opened: {row.workspace?.last_opened_at ?? 'unknown'}</p>
      {row.warning && <p role="alert">{row.warning}</p>}
      {row.database?.warning && <p role="alert">{row.database.warning}</p>}
    </div>
    {protectedRow && <button class="action-btn" disabled={disabled || !row.fingerprint} onClick={onReview}>Review deletion</button>}
  </li>;
}

/** Gateway-owned data uses the picker's existing gateway poll, rather than
 * the workspace SSE connection (which does not exist on this standalone page). */
export function CleanupManager({ tick, requestedWorkspace, onRequestHandled, onChanged }: {
  tick: number; requestedWorkspace: string | null; onRequestHandled: () => void; onChanged: () => Promise<void>;
}) {
  const open = useSignal(false);
  const review = useSignal<Loadable<CleanupReview>>({ status: 'not-loaded' });
  const trash = useSignal<Loadable<TrashEntry[]>>({ status: 'not-loaded' });
  const selected = useSignal<string[]>([]);
  const origin = useSignal('all');
  const frozen = useSignal<CleanupBatch | EmptyTrashBatch | null>(null);
  const emptying = useSignal(false);
  const phrase = useSignal('');
  const busy = useSignal(false);
  const error = useSignal('');
  const results = useSignal<CleanupResult[]>([]);
  const resultNames = useRef<Record<string, string>>({});
  const restoreChoice = useSignal<TrashEntry | null>(null);
  const abandon = useSignal(false);
  const pending = useSignal<CleanupRow[] | null>(null);
  const requested = useRef<string | null>(null);
  const seeded = useRef(false);
  const reading = useRef(false);
  const nudge = useSignal(false);
  const loading = useDelayedFlag(open.value && (review.value.status === 'loading' || trash.value.status === 'loading'));

  async function reload() {
    if (reading.current) return;
    reading.current = true;
    if (review.value.status !== 'loaded') review.value = { status: 'loading' };
    if (trash.value.status !== 'loaded') trash.value = { status: 'loading' };
    await Promise.all([
      getCleanupReview().then(data => {
        review.value = { status: 'loaded', data };
        const suggestions = data.rows.filter(row => isCleanupSuggested(row));
        if (!seeded.current && !data.refreshing) {
          selected.value = suggestions.map(row => row.id);
          seeded.current = true;
        }
        // Local storage limits a suggestion only. It never authorizes cleanup.
        try {
          const last = Number(localStorage.getItem('cleanup-nudge-seen') ?? 0);
          if (cleanupTotals(suggestions).knownBytes >= CLEANUP_NUDGE_BYTES && Date.now() - last >= CLEANUP_NUDGE_INTERVAL_MS) {
            nudge.value = true;
          }
        } catch { /* Storage denial leaves the review available through Manage. */ }
      }, reason => { review.value = toFailed(reason); }),
      listTrash().then(data => { trash.value = { status: 'loaded', data }; }, reason => { trash.value = toFailed(reason); }),
    ]);
    reading.current = false;
  }
  const data = review.value.status === 'loaded' ? review.value.data : null;
  // A requested deletion opens it, and a failed read must stay reachable.
  const shown = open.value || review.value.status === 'failed' || trash.value.status === 'failed'
    || (data !== null && trash.value.status === 'loaded' && hasCleanupWork(data.rows, trash.value.data));
  useEffect(() => { void reload(); }, []);
  // Hidden, it has no Refresh button, so the gateway poll is how new work appears.
  useEffect(() => {
    if (!shown || data?.refreshing) void reload();
  }, [tick]);
  useEffect(() => {
    if (!requestedWorkspace) { requested.current = null; return; }
    if (requested.current === requestedWorkspace) return;
    open.value = true;
    if (review.value.status === 'loaded' && !review.value.data.refreshing) {
      const target = review.value.data.rows.find(row => row.workspace?.id === requestedWorkspace);
      requested.current = requestedWorkspace;
      if (target) freeze([target]);
      else error.value = 'The workspace is absent from the current cleanup review. Refresh the review before trying again.';
      onRequestHandled();
    }
  }, [requestedWorkspace, review.value]);

  async function action(run: () => Promise<void>) {
    if (busy.value) return;
    busy.value = true;
    error.value = '';
    try { await run(); } catch (reason) { error.value = errorDetail(reason); }
    finally { busy.value = false; }
  }
  // Refresh observations without replacing the user's selected identities.
  // A refreshing server review is completed by the existing gateway poll.
  function freeze(rows: CleanupRow[]) {
    void action(async () => {
      const fresh = await getCleanupReview();
      pending.value = rows;
      review.value = { status: 'loaded', data: fresh };
    });
  }
  useEffect(() => {
    if (!pending.value || busy.value || review.value.status !== 'loaded' || review.value.data.refreshing) return;
    const rows = pending.value;
    pending.value = null;
    const fresh = review.value.data;
    void action(async () => {
      if (rows.some(row => !row.fingerprint || !fresh.rows.some(current =>
        current.id === row.id && current.fingerprint === row.fingerprint))) {
        throw new Error('Resources changed. Review the refreshed selection before trying again.');
      }
      emptying.value = false;
      frozen.value = await freezeCleanup(rows.map(row => ({ id: row.id, fingerprint: row.fingerprint! })));
      phrase.value = '';
      results.value = [];
    });
  }, [review.value, busy.value]);

  const chosen = data?.rows.filter(row => selected.value.includes(row.id)) ?? [];
  const totals = cleanupTotals(chosen);
  const batch = frozen.value;
  return <Disclosure open={shown}><section class="ws-cleanup" aria-label="Manage workspaces">
    <button class="action-btn" aria-expanded={open.value} onClick={() => {
      open.value = !open.value;
      if (open.value) {
        nudge.value = false;
        try { localStorage.setItem('cleanup-nudge-seen', String(Date.now())); } catch { /* Optional suggestion throttle. */ }
        void reload();
      }
    }}>Manage workspaces{nudge.value ? ' · Cleanup suggested' : ''}</button>
    <Disclosure open={open.value}>
      <h2>Manage workspaces</h2>
      <button class="action-btn" disabled={busy.value} onClick={() => { void reload(); }}>Refresh review</button>
      {error.value && <p role="alert">{error.value}</p>}
      <LoadingFade showSkeleton={loading} skeleton={<ListSkeletonOf row={() => <ReviewRow />} count={2} />}>
        {review.value.status === 'failed' && <LoadableError noun="cleanup review" error={review.value.error} onRetry={() => { void reload(); }} />}
        {review.value.status === 'not-loaded' && null}
        {review.value.status === 'loading' && null}
        {data && <>
          <p>{data.rows.length} resources · {formatBytes(cleanupTotals(data.rows).knownBytes)} known · {cleanupTotals(data.rows).unknownSizes} unknown sizes</p>
          {data.refreshing && <p>Sizes are being refreshed. Selection becomes available when observation completes.</p>}
          {data.warnings.map(warning => <p role="alert" key={warning}>{warning}</p>)}
          <div class="pill-bar">{['all', 'eval', 'e2e', 'demo', 'agent', 'user', 'unknown'].map(value => <button class="action-btn" aria-pressed={origin.value === value} onClick={() => { origin.value = value; }} key={value}>{value}</button>)}</div>
          <ul>{sortCleanupRows(data.rows).filter(row => origin.value === 'all' || (row.workspace?.origin ?? 'unknown') === origin.value).map(row =>
            <ReviewRow key={row.id} row={row} disabled={busy.value || data.refreshing || batch !== null}
              selected={selected.value.includes(row.id)} onSelect={() => {
                selected.value = selected.value.includes(row.id) ? selected.value.filter(id => id !== row.id) : [...selected.value, row.id];
              }} onReview={() => freeze([row])} />)}</ul>
          <p>Suggested selections include only stopped, observed stale harness workspaces. User and unknown workspaces require individual named confirmation.</p>
          <p>{totals.count} selected · {formatBytes(totals.knownBytes)} known · {totals.unknownSizes} unknown sizes</p>
          <button class="action-btn" disabled={busy.value || !!batch} onClick={() => { selected.value = []; }}>Clear</button>
          <button class="action-btn action-btn-danger" disabled={busy.value || !!batch || !chosen.length || data.refreshing} onClick={() => freeze(chosen)}>Review and delete</button>
        </>}
        {trash.value.status === 'failed' && <LoadableError noun="Trash" error={trash.value.error} onRetry={() => { void reload(); }} />}
        {trash.value.status === 'not-loaded' && null}
        {trash.value.status === 'loading' && null}
        {trash.value.status === 'loaded' && <>
          <h3>Trash</h3>
          {!trash.value.data.length && <p>Trash is empty.</p>}
          <ul>{trash.value.data.map(entry => <li class="list-row" key={entry.id}><div class="list-row-info">
            <strong>{entry.name}</strong><p>{entry.status} · {entry.bytes == null ? 'size unknown' : formatBytes(entry.bytes)}</p>
            {entry.warning && <p>{entry.warning}</p>}
            {!entry.restore_available && <p>Automatic restore is unavailable. An absent database dump cannot recover database contents.</p>}
          </div><button class="action-btn" disabled={busy.value || !entry.restore_available} onClick={() => { restoreChoice.value = entry; abandon.value = false; }}>Review restore</button>
            {entry.status === 'restoring' && <button class="action-btn" disabled={busy.value} onClick={() => { restoreChoice.value = entry; abandon.value = true; }}>Review staging reset</button>}
          </li>)}</ul>
          <button class="action-btn action-btn-danger" disabled={busy.value || !!batch || !trash.value.data.length} onClick={() => { void action(async () => {
            emptying.value = true; frozen.value = await freezeEmptyTrash(); phrase.value = ''; results.value = [];
          }); }}>Empty Trash</button>
        </>}
      </LoadingFade>
      <Disclosure open={batch !== null}>
        {batch && <>
          <h3>{emptying.value ? 'Permanently empty Trash' : 'Confirm cleanup'}</h3>
          <p>{emptying.value ? 'The frozen entries and their backups will be permanently removed.' : 'Workspaces stop, folders move to Trash, and databases are dropped. Eval, e2e and demo databases have no backup. User and unknown workspaces receive a database dump first.'}</p>
          <ul>{batch.members.map(member => <li key={member.id}>{'retained_branches' in member
            ? <>{member.workspace?.name ?? member.directory ?? member.database} · Retained branches: {member.retained_branches.join(', ') || 'none'}</>
            : <>{trash.value.status === 'loaded' ? trash.value.data.find(entry => entry.id === member.id)?.name ?? 'Trash entry' : 'Trash entry'} · {formatBytes(member.bytes)}</>}</li>)}</ul>
          {emptying.value && <p>Total: {formatBytes((batch as EmptyTrashBatch).members.reduce((sum, member) => sum + member.bytes, 0))}</p>}
          <p>Type exactly: <strong>{batch.phrase}</strong></p>
          <input autoFocus class="text-input" aria-label="Cleanup confirmation" value={phrase.value} onInput={event => { phrase.value = event.currentTarget.value; }} />
          <button class="action-btn" disabled={busy.value} onClick={() => { frozen.value = null; }}>Cancel</button>
          <button class="action-btn action-btn-danger" disabled={busy.value || phrase.value !== batch.phrase} onClick={() => { void action(async () => {
            const completed = emptying.value ? await confirmEmptyTrash(batch.id, phrase.value) : await confirmCleanup(batch.id, phrase.value);
            resultNames.current = Object.fromEntries(batch.members.map(member => [member.id,
              'retained_branches' in member ? member.workspace?.name ?? member.directory ?? member.database ?? 'Workspace'
                : trash.value.status === 'loaded' ? trash.value.data.find(entry => entry.id === member.id)?.name ?? 'Trash entry' : 'Trash entry']));
            results.value = completed.results;
            frozen.value = null; selected.value = [];
            await reload(); await onChanged();
          }); }}>Confirm {emptying.value ? 'Empty Trash' : 'deletion'}</button>
        </>}
      </Disclosure>
      <Disclosure open={restoreChoice.value !== null}>
        {restoreChoice.value && <>
          <h3>{abandon.value ? 'Reset interrupted restore staging' : 'Restore'}: {restoreChoice.value.name}</h3>
          <p>{restoreChoice.value.warning}</p>
          <p>{abandon.value ? 'The old staging database is retained. An occupied target or ownership-marker mismatch refuses reset. Retry restore after reset succeeds.' : 'Recovered worktrees use detached HEAD at the saved commit. Explicitly reattach each worktree to a branch after recovery. Restore does not start an engine.'}</p>
          <button class="action-btn" disabled={busy.value} onClick={() => { restoreChoice.value = null; }}>Cancel</button>
          <button class="action-btn" disabled={busy.value} onClick={() => { void action(async () => {
            const entry = restoreChoice.value!;
            if (abandon.value) await abandonTrashStaging(entry.id); else await restoreTrash(entry.id);
            restoreChoice.value = null; await reload(); await onChanged();
          }); }}>{abandon.value ? 'Reset staging, keep database' : 'Restore with detached worktrees'}</button>
        </>}
      </Disclosure>
      {results.value.length > 0 && <div role="status"><h3>Cleanup results</h3><ul>{results.value.map(result => <li key={result.id}>{resultNames.current[result.id]}: {result.status}{result.detail ? `: ${result.detail}` : ''}</li>)}</ul></div>}
    </Disclosure>
  </section></Disclosure>;
}
