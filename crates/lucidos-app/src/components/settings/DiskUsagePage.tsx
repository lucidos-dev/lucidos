import type { ComponentChildren } from 'preact';
import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { API, ApiError, mutatingFetch, throwIfNotOk } from '../../api/client';
import { showConfirm, showToast } from '../../store/store';
import { focusThreadOrBootstrap } from '../../store/actions/threads';
import { setLoadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText, SkeletonProvider, useSkeleton } from '../shared/Skeleton';
import { Explainer } from '../shared/Explainer';
import { errorDetail } from '../../utils/errorDetail';
import { formatTimeAgo } from '../../utils/formatTime';
import { formatBytes } from '../../utils/formatBytes';

export interface WorktreeRow {
  thread_id: string;
  thread_title: string | null;
  worktree_path: string;
  size_bytes: number;
  /** The part of `size_bytes` a build-artifact strip would free. */
  artifact_bytes: number;
  last_activity: string | null;
  is_dirty: boolean;
  is_saved: boolean;
  /** A coding agent or background task is working in the tree right now. */
  is_active: boolean;
  /** Nothing in the tree is lost by removing it. */
  is_finished: boolean;
}

interface InventoryResponse {
  worktrees: WorktreeRow[];
}

interface DiskSummary {
  free_bytes: number | null;
  total_bytes: number | null;
  workspace_data_bytes: number;
  soft_threshold_bytes: number;
  hard_threshold_bytes: number;
}

const inventory = signal<Loadable<WorktreeRow[]>>({ status: 'not-loaded' });
const summary = signal<Loadable<DiskSummary>>({ status: 'not-loaded' });

// Both loaders keep a loaded value on screen while they re-read. They run again
// after every cleanup, and blanking would unmount the page under the user.
async function loadSummary(): Promise<void> {
  setLoadingIfFresh(summary);
  try {
    const res = await fetch(`${API}/disk-usage/summary`);
    await throwIfNotOk(res);
    summary.value = { status: 'loaded', data: (await res.json()) as DiskSummary };
  } catch (e) {
    summary.value = toFailed(e);
  }
}

async function loadInventory(): Promise<void> {
  setLoadingIfFresh(inventory);
  try {
    const res = await fetch(`${API}/disk-usage/worktrees`);
    await throwIfNotOk(res);
    const body = (await res.json()) as InventoryResponse;
    inventory.value = { status: 'loaded', data: body.worktrees };
  } catch (e) {
    inventory.value = toFailed(e);
  }
  // Re-fetch summary so free disk + percentages stay in sync after cleanup.
  await loadSummary();
}

interface CleanupResult {
  tier: number;
  freed_bytes: number;
  branch_deleted: boolean;
}

/** Run one cleanup tier. Every refusal throws with the engine's reason. */
export async function runCleanup(threadId: string, tier: 1 | 2 | 3): Promise<CleanupResult> {
  const res = await mutatingFetch(
    `${API}/disk-usage/worktrees/${threadId}/cleanup`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tier }),
    },
  );
  await throwIfNotOk(res);
  return res.json();
}

/** Tier 2, or `null` when the engine refuses it with a 409, so the caller can
 *  offer the force of tier 3. */
export async function removeUnlessDirty(threadId: string): Promise<CleanupResult | null> {
  try {
    return await runCleanup(threadId, 2);
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 409) return null;
    throw e;
  }
}

interface RecommendedCleanupResult {
  removed_count: number;
  cleaned_count: number;
  freed_bytes: number;
}

/** The recommended cleanup over every worktree. The engine decides, per tree,
 *  what is safe at the moment it acts. */
export async function runRecommendedCleanup(): Promise<RecommendedCleanupResult> {
  const res = await mutatingFetch(`${API}/disk-usage/cleanup`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ action: 'recommended' }),
  });
  await throwIfNotOk(res);
  return res.json();
}

export interface RecommendedCleanupEstimate {
  removeCount: number;
  cleanCount: number;
  bytes: number;
}

/** What the recommended cleanup would free, by the inventory's figures: a
 *  finished tree whole, every other one its build artifacts. Live and pinned
 *  trees are skipped, as the engine skips them. */
export function estimateRecommendedCleanup(rows: WorktreeRow[]): RecommendedCleanupEstimate {
  const estimate = { removeCount: 0, cleanCount: 0, bytes: 0 };
  for (const row of rows) {
    if (row.is_active || row.is_saved) continue;
    if (row.is_finished) {
      estimate.removeCount += 1;
      estimate.bytes += row.size_bytes;
    } else if (row.artifact_bytes > 0) {
      estimate.cleanCount += 1;
      estimate.bytes += row.artifact_bytes;
    }
  }
  return estimate;
}

function countOf(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? '' : 's'}`;
}

/** One sentence naming what the recommended cleanup touches. */
export function describeEstimate({ removeCount, cleanCount }: RecommendedCleanupEstimate): string {
  const parts = [
    removeCount > 0 && `removes ${countOf(removeCount, 'finished worktree')}`,
    cleanCount > 0 && `clears build artifacts in ${countOf(cleanCount, 'worktree')}`,
  ].filter(Boolean);
  const action = parts.join(' and ');
  return `${action.charAt(0).toUpperCase()}${action.slice(1)}. Uncommitted and unmerged work stays, and it skips running or pinned threads.`;
}

interface PressureLabel {
  text: string;
  cls: string;
}

function pressureLabelFor(sum: DiskSummary | null): PressureLabel | null {
  if (!sum || sum.free_bytes == null) return null;
  if (sum.free_bytes < sum.hard_threshold_bytes) {
    return { text: 'Disk is critical', cls: 'disk-usage-pressure-hard' };
  }
  if (sum.free_bytes < sum.soft_threshold_bytes) {
    return { text: 'Getting low', cls: 'disk-usage-pressure-soft' };
  }
  return null;
}

// Palette cycled per worktree segment; matches the swatch on each row below.
const SEGMENT_PALETTE = ['c0', 'c1', 'c2', 'c3', 'c4', 'c5'] as const;

function paletteClass(idx: number): string {
  return `disk-usage-segment-${SEGMENT_PALETTE[idx % SEGMENT_PALETTE.length]}`;
}

function Segment({ cls, bytes, label }: { cls: string; bytes: number; label: string }) {
  if (bytes <= 0) return null;
  return (
    <div
      class={`disk-usage-segment ${cls}`}
      style={{ flexGrow: bytes }}
      data-tooltip={`${label}: ${formatBytes(bytes)}`}
    />
  );
}

/** The volume's breakdown as one bar. Inside a `SkeletonProvider` it draws the
 *  bar's own box with a shimmer across it. */
function DiskUsageBar({
  rows = [],
  freeBytes = 0,
  otherBytes = 0,
  workspaceDataBytes = 0,
}: {
  rows?: WorktreeRow[];
  freeBytes?: number;
  otherBytes?: number;
  workspaceDataBytes?: number;
}) {
  if (useSkeleton()) {
    return <div class="disk-usage-bar" aria-hidden="true"><SkBlock w="100%" h="100%" /></div>;
  }
  return (
    <div class="disk-usage-bar" role="img" aria-label="Disk usage breakdown">
      {rows.map((row, idx) => (
        <Segment
          key={row.thread_id}
          cls={paletteClass(idx)}
          bytes={row.size_bytes}
          label={row.thread_title?.trim() || 'Untitled thread'}
        />
      ))}
      <Segment
        cls="disk-usage-segment-workspace-data"
        bytes={workspaceDataBytes}
        label="Workspace data"
      />
      <Segment cls="disk-usage-segment-other" bytes={otherBytes} label="Other apps" />
      <Segment cls="disk-usage-segment-free" bytes={freeBytes} label="Free" />
    </div>
  );
}

function skeletonBar(w: string) {
  return <SkeletonProvider><SkText w={w} /></SkeletonProvider>;
}

/** A figure that shimmers while its read is pending. */
function Figure({
  skeleton,
  w = '4rem',
  children,
}: {
  skeleton: boolean;
  w?: string;
  children: ComponentChildren;
}) {
  return (
    <span class="disk-usage-figure">
      <LoadingFade showSkeleton={skeleton} skeleton={skeletonBar(w)}>{children}</LoadingFade>
    </span>
  );
}

/** One line of the key under the bar: a swatch, what it is, how much. */
function KeyRow({
  swatch,
  label,
  explainer,
  note,
  children,
}: {
  swatch: string;
  label: string;
  explainer?: ComponentChildren;
  note?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <div class="disk-usage-key-row">
      <span class={`disk-usage-legend-swatch ${swatch}`} aria-hidden="true" />
      <span class="disk-usage-key-label">
        {label}
        {explainer}
        {note && <span class="disk-usage-key-note">{note}</span>}
      </span>
      <span class="disk-usage-key-value">{children}</span>
    </div>
  );
}

/** The "Free up space" card. With no `rows` its figure shimmers. */
function RecommendedCleanupCard({
  rows,
  skeleton,
  onDone,
}: {
  rows: WorktreeRow[] | null;
  skeleton: boolean;
  onDone: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const estimate = rows ? estimateRecommendedCleanup(rows) : null;
  const nothingToFree = estimate?.bytes === 0;

  async function handleFree() {
    if (!estimate) return;
    if (!(await showConfirm(
      `Free about ${formatBytes(estimate.bytes)}?\n\n${describeEstimate(estimate)} Those threads rebuild on their next turn.`,
      'Free space',
      { variant: 'default' },
    ))) {
      return;
    }
    setBusy(true);
    try {
      const result = await runRecommendedCleanup();
      const done = [
        result.removed_count > 0 && `removed ${countOf(result.removed_count, 'finished worktree')}`,
        result.cleaned_count > 0 && `cleared build artifacts in ${countOf(result.cleaned_count, 'worktree')}`,
      ].filter(Boolean).join(' and ');
      showToast(`Freed ${formatBytes(result.freed_bytes)}${done ? `: ${done}` : ''}`, 'success');
      await onDone();
    } catch (e) {
      showToast(`Cleanup failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="disk-usage-reclaim">
      <div class="disk-usage-reclaim-text">
        <div class="disk-usage-reclaim-title">Free up space</div>
        <div class="disk-usage-reclaim-figure">
          <Figure skeleton={skeleton} w="9rem">
            {estimate && (nothingToFree
              ? 'Nothing to free right now'
              : `About ${formatBytes(estimate.bytes)} can be freed`)}
          </Figure>
        </div>
        {estimate && !nothingToFree && (
          <p class="disk-usage-reclaim-note">{describeEstimate(estimate)}</p>
        )}
      </div>
      {estimate && !nothingToFree && (
        <button class="action-btn disk-usage-reclaim-btn" disabled={busy} onClick={handleFree}>
          {busy ? 'Freeing…' : `Free ${formatBytes(estimate.bytes)}`}
        </button>
      )}
    </div>
  );
}

function StatusBadge({ row }: { row: WorktreeRow }) {
  if (row.is_active) return <span class="label" data-tooltip="A coding agent or task is working in this worktree">Running</span>;
  if (row.is_saved) return <span class="label" data-tooltip="Pinned threads are exempt from auto-cleanup">Pinned</span>;
  if (row.is_dirty) return <span class="label channel-error" data-tooltip="Worktree has uncommitted changes">Dirty</span>;
  if (row.is_finished) return <span class="label" data-tooltip="Everything here is already on main; removing it loses nothing">Finished</span>;
  return <span class="label" data-tooltip="No uncommitted changes">Clean</span>;
}

/** One worktree. With no `row`, inside a `SkeletonProvider`, it draws itself
 *  as the loading placeholder. */
function WorktreeRowView({
  row,
  totalBytes = 0,
  colorIdx,
  onCleanup = () => {},
}: {
  row?: WorktreeRow;
  totalBytes?: number;
  colorIdx: number;
  onCleanup?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const title = row?.thread_title?.trim() || 'Untitled thread';
  const last = row?.last_activity ? formatTimeAgo(new Date(row.last_activity)) : 'no activity';
  const pct = row && totalBytes > 0 ? Math.round((row.size_bytes / totalBytes) * 100) : 0;
  const disabled = busy || !!row?.is_active;

  async function handleClean() {
    if (!row) return;
    setBusy(true);
    try {
      const result = await runCleanup(row.thread_id, 1);
      showToast(`Freed ${formatBytes(result.freed_bytes)} from build artifacts`, 'success');
      onCleanup();
    } catch (e) {
      showToast(`Cleanup failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    if (!row) return;
    if (!(await showConfirm(
      `Remove worktree for "${title}"? Its on-disk files (including any build artifacts) will be deleted. Branch is kept if it has unmerged commits.`,
      'Remove',
      { variant: 'danger' },
    ))) {
      return;
    }
    setBusy(true);
    try {
      let result = await removeUnlessDirty(row.thread_id);
      if (!result) {
        // Dirty worktree — confirm again, then force via Tier 3
        if (!(await showConfirm(
          `"${title}" has uncommitted changes in its worktree. Force-remove anyway? Uncommitted edits will be lost.`,
          'Force remove',
          { variant: 'danger' },
        ))) {
          return;
        }
        result = await runCleanup(row.thread_id, 3);
      }
      showToast(`Removed worktree (freed ${formatBytes(result.freed_bytes)}${result.branch_deleted ? '; branch deleted' : ''})`, 'success');
      onCleanup();
    } catch (e) {
      showToast(`Remove failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  const swatch = (
    <span
      class={`disk-usage-legend-swatch ${paletteClass(colorIdx)}`}
      aria-hidden="true"
    />
  );

  return (
    <div class="list-row disk-usage-row">
      <div class="list-row-info">
        {/* The swatch sits INSIDE the button. A button is an atomic inline, so
            beside it the whole title would drop below the swatch on a wrap. */}
        <div class="title list-row-name">
          {row ? (
            <button type="button" class="accent-link" onClick={() => focusThreadOrBootstrap(row.thread_id)}>
              {swatch}{title}
            </button>
          ) : <>{swatch}<SkText w="10rem" /></>}
        </div>
        {/* Four FIELDS, separated by `.list-row-details`' own 0.75rem flex gap.
            No manual "·" glue: each dot would be its own anonymous flex item
            and pick the gap up on both sides, double-spacing the row. */}
        {row ? (
          <div class="list-row-details">
            <span data-tooltip={row.worktree_path}>{formatBytes(row.size_bytes)}</span>
            <span data-tooltip="Share of total worktree disk usage">{pct}%</span>
            <span>{last}</span>
            <StatusBadge row={row} />
          </div>
        ) : <SkText class="list-row-details" as="div" w="14rem" />}
      </div>
      <div class="list-row-actions">
        <SkBlock w="6.5rem" h="1.5rem" round>
          <button class="action-btn action-btn-secondary" disabled={disabled} onClick={handleClean}>
            {busy ? '...' : 'Clean artifacts'}
          </button>
        </SkBlock>
        <SkBlock w="4.5rem" h="1.5rem" round>
          <button class="action-btn action-btn-danger" disabled={disabled} onClick={handleRemove}>
            {busy ? '...' : 'Remove'}
          </button>
        </SkBlock>
      </div>
    </div>
  );
}

/** Whether opening the page owes a read. A failed read counts: the signals are
 *  module-level, so a one-off failure would otherwise stick until a reload. */
export function diskUsageLoadOwed(loadable: Loadable<unknown>): boolean {
  return loadable.status === 'not-loaded' || loadable.status === 'failed';
}

function isPending(loadable: Loadable<unknown>): boolean {
  return loadable.status === 'not-loaded' || loadable.status === 'loading';
}

export function DiskUsagePage() {
  const loadable = inventory.value;
  const summaryLoadable = summary.value;
  const inventoryPending = isPending(loadable);
  const summaryPending = isPending(summaryLoadable);
  // One gate for both reads, so their skeletons arrive in one wave. Each slot
  // still clears on its own read.
  const gate = useDelayedFlag(inventoryPending || summaryPending);

  useEffect(() => {
    if (diskUsageLoadOwed(loadable)) void loadInventory();
    if (diskUsageLoadOwed(summaryLoadable)) void loadSummary();
  }, []);
  // Re-reads the worktrees, then the summary, keeping both on screen meanwhile.
  usePanelRefresh('disk usage', loadInventory);

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="list-rows">
          <LoadableError noun="disk usage" error={loadable.error} onRetry={() => void loadInventory()} />
        </div>
      </div>
    );
  }

  const rows = loadable.status === 'loaded' ? loadable.data : null;
  const totalBytes = rows?.reduce((acc, r) => acc + r.size_bytes, 0) ?? 0;
  const sum = summaryLoadable.status === 'loaded' ? summaryLoadable.data : null;
  const freeBytes = sum?.free_bytes ?? null;
  const diskTotalBytes = sum?.total_bytes ?? null;
  const workspaceDataBytes = sum?.workspace_data_bytes ?? null;
  const otherBytes = rows && freeBytes != null && diskTotalBytes != null
    ? Math.max(0, diskTotalBytes - freeBytes - totalBytes - (workspaceDataBytes ?? 0))
    : null;
  const pressureLabel = pressureLabelFor(sum);
  // A settled summary with no figure reads as a dash; a pending one is empty.
  const dash = summaryPending ? null : '–';
  const summarySkeleton = gate && summaryPending;
  const inventorySkeleton = gate && inventoryPending;
  const bothSkeleton = gate && (inventoryPending || summaryPending);
  const barOwed = inventoryPending || summaryPending || (freeBytes != null && diskTotalBytes != null);

  return (
    <>
      <div class="settings-section disk-usage-overview">
        <div class="disk-usage-headline">
          <span class="disk-usage-headline-value">
            <Figure skeleton={summarySkeleton} w="5rem">{freeBytes != null ? formatBytes(freeBytes) : dash}</Figure>
          </span>
          <span class="disk-usage-headline-sub">
            free{diskTotalBytes != null && ` of ${formatBytes(diskTotalBytes)}`}
          </span>
          {pressureLabel && <span class={`disk-usage-pressure ${pressureLabel.cls}`}>{pressureLabel.text}</span>}
        </div>
        {summaryLoadable.status === 'failed' && (
          <p class="settings-section-desc error-text">
            Couldn't read disk stats: {summaryLoadable.error}
          </p>
        )}
        {barOwed && (
          <LoadingFade
            showSkeleton={bothSkeleton}
            skeleton={<SkeletonProvider><DiskUsageBar /></SkeletonProvider>}
          >
            {rows && freeBytes != null && otherBytes != null && (
              <DiskUsageBar
                rows={rows}
                freeBytes={freeBytes}
                otherBytes={otherBytes}
                workspaceDataBytes={workspaceDataBytes ?? 0}
              />
            )}
          </LoadingFade>
        )}
        <div class="disk-usage-key">
          <KeyRow
            swatch="disk-usage-segment-worktrees"
            label="Worktrees"
            note={rows && rows.length > 0 && countOf(rows.length, 'worktree')}
            explainer={
              <Explainer title="Worktrees">
                <p>Per-thread worktrees Lucidos creates for coding-agent sessions.</p>
                <p>
                  Clean build artifacts to reclaim space, or remove a worktree entirely
                  when you no longer need its branch.
                </p>
              </Explainer>
            }
          >
            <Figure skeleton={inventorySkeleton}>{rows && formatBytes(totalBytes)}</Figure>
          </KeyRow>
          <KeyRow
            swatch="disk-usage-segment-workspace-data"
            label="Workspace data"
            explainer={
              <Explainer title="Workspace data">
                <p>
                  The workspace's own data directory: artifacts you and the engine produce,
                  the Postgres event store, and your apps &amp; knowhow.
                </p>
                <p>Kept across restarts; no automatic cleanup.</p>
              </Explainer>
            }
          >
            <Figure skeleton={summarySkeleton}>{workspaceDataBytes != null ? formatBytes(workspaceDataBytes) : dash}</Figure>
          </KeyRow>
          <KeyRow swatch="disk-usage-segment-other" label="Other apps">
            <Figure skeleton={bothSkeleton}>{otherBytes != null ? formatBytes(otherBytes) : !inventoryPending && dash}</Figure>
          </KeyRow>
        </div>
      </div>

      <div class="settings-section">
        <RecommendedCleanupCard rows={rows} skeleton={inventorySkeleton} onDone={loadInventory} />
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Worktrees</div>
        {rows?.length === 0 && <p class="settings-section-desc">No worktrees on disk</p>}
        {/* Unmounts WITH its content once an empty list settles, so no empty
            fade box is left behind. */}
        {(inventoryPending || (rows?.length ?? 0) > 0) && (
          <LoadingFade
            showSkeleton={inventorySkeleton}
            skeleton={<ListSkeletonOf count={3} containerClass="list-rows disk-usage-rows" row={(i) => <WorktreeRowView colorIdx={i} />} />}
          >
            {rows && (
              <div class="list-rows disk-usage-rows">
                {rows.map((row, idx) => (
                  <WorktreeRowView
                    key={row.thread_id}
                    row={row}
                    totalBytes={totalBytes}
                    colorIdx={idx}
                    onCleanup={loadInventory}
                  />
                ))}
              </div>
            )}
          </LoadingFade>
        )}
      </div>
    </>
  );
}
