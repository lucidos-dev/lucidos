import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { API_BASE } from '../../api/client';
import { showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { errorDetail } from '../../utils/errorDetail';
import { formatTimeAgo } from '../../utils/formatTime';

/**
 * Settings → Disk Usage page (Phase 10.4 of the CC resume architecture).
 *
 * Shows the same per-thread worktree inventory the background cleanup worker
 * sees, so the user can reclaim disk space on demand without waiting for the
 * hourly tick. Per-row actions:
 *
 * - **Clean build artifacts** (Tier 1): strips `target/`, `node_modules/`,
 *   `.lucidos/cache/`. Always safe.
 * - **Remove worktree** (Tier 2 / Tier 3): removes the entire worktree dir
 *   and deletes the branch when fully merged. The Tier 2 endpoint refuses on
 *   dirty worktrees; in that case the UI re-prompts and falls back to Tier 3
 *   (force) after explicit confirmation.
 */

interface WorktreeRow {
  thread_id: string;
  thread_title: string | null;
  worktree_path: string;
  size_bytes: number;
  last_activity: string | null;
  is_dirty: boolean;
  is_pinned: boolean;
}

interface InventoryResponse {
  worktrees: WorktreeRow[];
}

interface DiskSummary {
  free_bytes: number | null;
  total_bytes: number | null;
  soft_threshold_bytes: number;
  hard_threshold_bytes: number;
}

const inventory = signal<Loadable<WorktreeRow[]>>({ status: 'not-loaded' });
const summary = signal<Loadable<DiskSummary>>({ status: 'not-loaded' });

async function loadSummary(): Promise<void> {
  summary.value = { status: 'loading' };
  try {
    const res = await fetch(`${API_BASE}/api/disk-usage/summary`);
    if (!res.ok) {
      throw new Error(`HTTP ${res.status}: ${await res.text()}`);
    }
    summary.value = { status: 'loaded', data: (await res.json()) as DiskSummary };
  } catch (e) {
    summary.value = toFailed(e);
  }
}

async function loadInventory(): Promise<void> {
  inventory.value = { status: 'loading' };
  try {
    const res = await fetch(`${API_BASE}/api/disk-usage/worktrees`);
    if (!res.ok) {
      throw new Error(`HTTP ${res.status}: ${await res.text()}`);
    }
    const body = (await res.json()) as InventoryResponse;
    inventory.value = { status: 'loaded', data: body.worktrees };
  } catch (e) {
    inventory.value = toFailed(e);
  }
  // Re-fetch summary so free disk + percentages stay in sync after cleanup.
  await loadSummary();
}

async function runCleanup(threadId: string, tier: 1 | 2 | 3): Promise<{ tier: number; freed_bytes: number; branch_deleted: boolean } | null> {
  const res = await fetch(
    `${API_BASE}/api/disk-usage/worktrees/${threadId}/cleanup`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tier }),
    },
  );
  if (res.status === 409) {
    return null; // dirty — caller decides whether to retry as Tier 3
  }
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  }
  return res.json();
}

/** Pretty-print a byte count in the largest sensible unit. */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes / 1024;
  let unitIdx = 0;
  while (value >= 1024 && unitIdx < units.length - 1) {
    value /= 1024;
    unitIdx += 1;
  }
  // Show one decimal up to the GB range, none for TB
  return `${value.toFixed(unitIdx >= 2 ? 1 : 0)} ${units[unitIdx]}`;
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
      data-tooltip={`${label} — ${formatBytes(bytes)}`}
    />
  );
}

function DiskUsageBar({
  rows,
  totalBytes,
  freeBytes,
  diskTotalBytes,
}: {
  rows: WorktreeRow[];
  totalBytes: number;
  freeBytes: number;
  diskTotalBytes: number;
}) {
  const otherUsed = Math.max(0, diskTotalBytes - freeBytes - totalBytes);
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
      <Segment cls="disk-usage-segment-other" bytes={otherUsed} label="Other apps" />
      <Segment cls="disk-usage-segment-free" bytes={freeBytes} label="Free" />
    </div>
  );
}

function StatusBadge({ row }: { row: WorktreeRow }) {
  if (row.is_pinned) return <span class="label" data-tooltip="Pinned threads are exempt from auto-cleanup">Pinned</span>;
  if (row.is_dirty) return <span class="label channel-error" data-tooltip="Worktree has uncommitted changes">Dirty</span>;
  return <span class="label" data-tooltip="No uncommitted changes; safe to remove">Clean</span>;
}

function WorktreeRowView({
  row,
  totalBytes,
  colorIdx,
  onCleanup,
}: {
  row: WorktreeRow;
  totalBytes: number;
  colorIdx: number;
  onCleanup: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const title = row.thread_title?.trim() || 'Untitled thread';
  const last = row.last_activity ? formatTimeAgo(new Date(row.last_activity)) : 'no activity';
  const pct = totalBytes > 0 ? Math.round((row.size_bytes / totalBytes) * 100) : 0;

  async function handleClean() {
    setBusy(true);
    try {
      const result = await runCleanup(row.thread_id, 1);
      if (result) {
        showToast(`Freed ${formatBytes(result.freed_bytes)} from build artifacts`, 'success');
        onCleanup();
      } else {
        showToast('Nothing to clean', 'info');
      }
    } catch (e) {
      showToast(`Cleanup failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove() {
    if (!(await showConfirm(
      `Remove worktree for "${title}"? Build artifacts and its on-disk files will be deleted. Branch is kept if it has unmerged commits.`,
      'Remove',
    ))) {
      return;
    }
    setBusy(true);
    try {
      let result = await runCleanup(row.thread_id, 2);
      if (!result) {
        // Dirty worktree — confirm again, then force via Tier 3
        if (!(await showConfirm(
          `"${title}" has uncommitted changes in its worktree. Force-remove anyway? Uncommitted edits will be lost.`,
          'Force remove',
        ))) {
          return;
        }
        result = await runCleanup(row.thread_id, 3);
      }
      if (result) {
        showToast(`Removed worktree (freed ${formatBytes(result.freed_bytes)}${result.branch_deleted ? '; branch deleted' : ''})`, 'success');
        onCleanup();
      }
    } catch (e) {
      showToast(`Remove failed: ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="list-row disk-usage-row">
      <div class="list-row-info">
        <div class="title list-row-name">
          <span
            class={`disk-usage-legend-swatch ${paletteClass(colorIdx)}`}
            aria-hidden="true"
          />
          {' '}
          {title}
        </div>
        <div class="list-row-details">
          <span data-tooltip={row.worktree_path}>{formatBytes(row.size_bytes)}</span>
          {' · '}
          <span data-tooltip="Share of total worktree disk usage">{pct}%</span>
          {' · '}
          <span>{last}</span>
          {' · '}
          <StatusBadge row={row} />
        </div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn" disabled={busy} onClick={handleClean}>
          {busy ? '...' : 'Clean build artifacts'}
        </button>
        <button class="action-btn action-btn-danger" disabled={busy} onClick={handleRemove}>
          {busy ? '...' : 'Remove worktree'}
        </button>
      </div>
    </div>
  );
}

export function DiskUsagePage() {
  const loadable = inventory.value;
  const summaryLoadable = summary.value;
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    if (loadable.status === 'not-loaded') loadInventory();
    if (summaryLoadable.status === 'not-loaded') loadSummary();
  }, []);

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="settings-section-title">Worktrees</div>
        <div class="list-rows">
          <div class="empty-state error-text">Failed to load disk usage: {loadable.error}</div>
        </div>
      </div>
    );
  }

  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="settings-section">
        <div class="settings-section-title">Worktrees</div>
        <div class="list-rows">
          <div class="empty">Loading...</div>
        </div>
      </div>
    );
  }

  const totalBytes = loadable.data.reduce((acc, r) => acc + r.size_bytes, 0);
  const sum = summaryLoadable.status === 'loaded' ? summaryLoadable.data : null;
  const freeBytes = sum?.free_bytes ?? null;
  const diskTotalBytes = sum?.total_bytes ?? null;

  return (
    <div class="settings-section">
      <div class="settings-section-title">Worktrees</div>
      <p class="settings-section-desc">
        {sum && sum.free_bytes != null && sum.total_bytes != null ? (
          <>
            <strong>{formatBytes(sum.free_bytes)} free</strong> of {formatBytes(sum.total_bytes)} on this volume.
            Lucidos worktrees use {formatBytes(totalBytes)}.
            {sum.free_bytes < sum.hard_threshold_bytes && (
              <span class="error-text"> Disk is critical — Lucidos is reclaiming idle worktrees aggressively.</span>
            )}
            {sum.free_bytes >= sum.hard_threshold_bytes && sum.free_bytes < sum.soft_threshold_bytes && ' Disk is getting low.'}
          </>
        ) : (
          <>Lucidos worktrees use {formatBytes(totalBytes)} on disk.</>
        )}
        {summaryLoadable.status === 'failed' && (
          <span class="error-text"> Couldn't read disk stats: {summaryLoadable.error}</span>
        )}
        {' '}
        Clean build artifacts to reclaim space; remove a worktree entirely when you no longer need its branch.
      </p>
      {freeBytes != null && diskTotalBytes != null && (
        <DiskUsageBar
          rows={loadable.data}
          totalBytes={totalBytes}
          freeBytes={freeBytes}
          diskTotalBytes={diskTotalBytes}
        />
      )}
      <div class="list-rows">
        {loadable.data.length === 0 ? (
          <div class="empty">No worktrees</div>
        ) : (
          loadable.data.map((row, idx) => (
            <WorktreeRowView
              key={row.thread_id}
              row={row}
              totalBytes={totalBytes}
              colorIdx={idx}
              onCleanup={loadInventory}
            />
          ))
        )}
      </div>
    </div>
  );
}
