
import { signal } from '@preact/signals';
import {
  archiveAll,
  archiveAllPreflight,
  unarchiveThreads,
  type ArchiveAllKept,
  type ArchiveAllPreflight,
} from '../../api/threads';
import { showConfirm, showToast } from '../store';
import type { ToastAction } from '../types';
import { errorDetail } from '../../utils/errorDetail';
import { CHANGE_TO_RESOLVE, WAITING_FOR_ANSWER } from './blockerCopy';
import { ARCHIVE_ALL_MAX_IDS, ARCHIVE_ALL_PINNED_SUB_THREAD, ARCHIVE_ALL_SAME_FAMILY } from '@lucidos/engine-constants';

const UNDO_MS = 10_000;

export function idBatches(ids: string[]): string[][] {
  const batches: string[][] = [];
  for (let i = 0; i < ids.length; i += ARCHIVE_ALL_MAX_IDS) {
    batches.push(ids.slice(i, i + ARCHIVE_ALL_MAX_IDS));
  }
  return batches;
}

export const archiveAllPhase = signal<'idle' | 'checking' | 'confirming' | 'archiving'>('idle');

const KEPT_PHRASES: Record<string, [string, string]> = {
  question: [WAITING_FOR_ANSWER, WAITING_FOR_ANSWER],
  pending_change: [`with ${CHANGE_TO_RESOLVE}`, `with ${CHANGE_TO_RESOLVE}`],
  unproposed_work: ['with unproposed work', 'with unproposed work'],
  draft: ['with an unsent draft', 'with unsent drafts'],
  failed_run: ['whose last run failed', 'whose last run failed'],
  busy: ['still working', 'still working'],
  [ARCHIVE_ALL_PINNED_SUB_THREAD]: ['pinned', 'pinned'],
  pinned: ['now pinned', 'now pinned'],
  archived: ['already archived', 'already archived'],
  gone: ['no longer there', 'no longer there'],
  [ARCHIVE_ALL_SAME_FAMILY]: ['more alongside them', 'more alongside them'],
};

const slugRank = (slug: string) => {
  const rank = Object.keys(KEPT_PHRASES).indexOf(slug);
  return rank === -1 ? Object.keys(KEPT_PHRASES).length : rank;
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function keptSummary(counts: Record<string, number>): string {
  return Object.entries(counts)
    .filter(([, n]) => n > 0)
    .sort(([a], [b]) => slugRank(a) - slugRank(b))
    .map(([slug, n]) => {
      const [one, many] = KEPT_PHRASES[slug] ?? ['kept for another reason', 'kept for another reason'];
      return plural(n, one, many);
    })
    .join(', ');
}

export function archiveAllConfirmation(
  preflight: ArchiveAllPreflight,
): { title: string; message: string } | null {
  if (preflight.safe.length === 0) return null;
  const title = `Archive ${plural(preflight.safe_thread_count, 'thread', 'threads')}?`;
  const kept = preflight.kept_thread_count;
  if (kept === 0) {
    return { title, message: 'Everything in Current goes to Archive. You can undo right after.' };
  }
  const stay = kept === 1 ? '1 stays open' : `${kept} stay open`;
  return { title, message: `${stay}: ${keptSummary(preflight.kept)}. You can undo right after.` };
}

export function nothingToArchiveMessage(preflight: ArchiveAllPreflight): string {
  const kept = preflight.kept_thread_count;
  if (kept === 0) return 'Nothing to archive.';
  const stay = kept === 1 ? '1 thread stays open' : `${kept} threads stay open`;
  return `Nothing to archive. ${stay}: ${keptSummary(preflight.kept)}.`;
}

export function changedSinceConfirm(kept: ArchiveAllKept[]): Record<string, number> {
  const changed: Record<string, number> = {};
  const add = (slug: string, n: number) => {
    if (n > 0) changed[slug] = (changed[slug] ?? 0) + n;
  };
  for (const { slug, thread_count } of kept) {
    add(slug, 1);
    add(ARCHIVE_ALL_SAME_FAMILY, thread_count - 1);
  }
  return changed;
}

export function archiveAllResultMessage(archived: number, changed: Record<string, number>): string {
  const done = `Archived ${plural(archived, 'thread', 'threads')}.`;
  const total = Object.values(changed).reduce((a, b) => a + b, 0);
  return total === 0 ? done : `${done} ${total} changed since you confirmed and stayed: ${keptSummary(changed)}.`;
}

export async function handleArchiveAll(): Promise<void> {
  if (archiveAllPhase.value !== 'idle') return;
  archiveAllPhase.value = 'checking';
  try {
    await runArchiveAll();
  } finally {
    archiveAllPhase.value = 'idle';
  }
}

async function runArchiveAll(): Promise<void> {
  let preflight: ArchiveAllPreflight;
  try {
    preflight = await archiveAllPreflight();
  } catch (e) {
    showToast(`Could not check what to archive: ${errorDetail(e)}`, 'error');
    return;
  }
  const confirmation = archiveAllConfirmation(preflight);
  if (!confirmation) {
    showToast(nothingToArchiveMessage(preflight), 'info');
    return;
  }
  archiveAllPhase.value = 'confirming';
  const ok = await showConfirm(confirmation.message, 'Archive', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'default',
  });
  if (!ok) return;
  archiveAllPhase.value = 'archiving';

  const batch: string[] = [];
  const kept: ArchiveAllKept[] = [];
  try {
    for (const ids of idBatches(preflight.safe.map((t) => t.thread_id))) {
      const result = await archiveAll(ids);
      batch.push(...result.archived);
      kept.push(...result.kept);
    }
  } catch (e) {
    if (batch.length === 0) {
      showToast(`Archive all failed: ${errorDetail(e)}`, 'error');
    } else {
      showToast(`Archive all stopped after ${plural(batch.length, 'thread', 'threads')}: ${errorDetail(e)}`, 'error', {
        action: undoAction(batch),
      });
    }
    return;
  }
  showToast(archiveAllResultMessage(batch.length, changedSinceConfirm(kept)), 'success', {
    autoDismissMs: UNDO_MS,
    action: batch.length > 0 ? undoAction(batch) : undefined,
  });
}

export function undoAction(batch: string[]): ToastAction {
  return { label: 'Undo', onClick: () => void undoArchiveAll(batch), deliberate: true };
}

async function undoArchiveAll(batch: string[]): Promise<void> {
  try {
    let restored = 0;
    for (const ids of idBatches(batch)) {
      restored += (await unarchiveThreads(ids)).unarchived.length;
    }
    showToast(`Restored ${plural(restored, 'thread', 'threads')} to Current.`, 'info');
  } catch (e) {
    showToast(`Undo failed: ${errorDetail(e)}`, 'error');
  }
}
