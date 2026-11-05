/** Archive all: the Current section's bulk archive, with its safety net
 *  (ADR 0349).
 *
 *  The engine decides what is safe. The preflight lists the threads that can
 *  go and counts the ones that stay, and the press sends back exactly the
 *  listed ids. The result toast carries Undo, which unarchives that batch.
 */

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

/** How long the Undo stays on screen. */
const UNDO_MS = 10_000;

/** Split `ids` into requests the engine accepts. */
export function idBatches(ids: string[]): string[][] {
  const batches: string[][] = [];
  for (let i = 0; i < ids.length; i += ARCHIVE_ALL_MAX_IDS) {
    batches.push(ids.slice(i, i + ARCHIVE_ALL_MAX_IDS));
  }
  return batches;
}

/** A press still running, so a second press cannot start a parallel one. */
let pressInFlight = false;

/** Each kept reason as a count phrase, singular then plural, in the order the
 *  summary names them. Where a reason is also a thread menu blocker, the
 *  phrase is the menu's own words (ADR 0378). `busy` is wider than running: it
 *  covers a paused turn and an event wait. */
const KEPT_PHRASES: Record<string, [string, string]> = {
  question: [WAITING_FOR_ANSWER, WAITING_FOR_ANSWER],
  pending_change: [`with ${CHANGE_TO_RESOLVE}`, `with ${CHANGE_TO_RESOLVE}`],
  unproposed_work: ['with unproposed work', 'with unproposed work'],
  draft: ['with an unsent draft', 'with unsent drafts'],
  failed_run: ['whose last run failed', 'whose last run failed'],
  busy: ['still working', 'still working'],
  // A pinned sub-thread under a safe root: the archive leaves it open.
  [ARCHIVE_ALL_PINNED_SUB_THREAD]: ['pinned', 'pinned'],
  // Only a thread that changed after the confirm is kept for these three.
  pinned: ['now pinned', 'now pinned'],
  archived: ['already archived', 'already archived'],
  gone: ['no longer there', 'no longer there'],
  // The other inbox threads of a family kept for one of the reasons above.
  [ARCHIVE_ALL_SAME_FAMILY]: ['more alongside them', 'more alongside them'],
};

/** Where a slug sits in the summary: `KEPT_PHRASES` order, unknown slugs last. */
const slugRank = (slug: string) => {
  const rank = Object.keys(KEPT_PHRASES).indexOf(slug);
  return rank === -1 ? Object.keys(KEPT_PHRASES).length : rank;
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The kept threads as "12 waiting for your answer, 4 still working". */
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

/** The confirm, or `null` when nothing in Current is safe to archive. Pure,
 *  so every line of the copy is testable without a dialog. */
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

/** What the toast says when nothing in Current is safe to archive. */
export function nothingToArchiveMessage(preflight: ArchiveAllPreflight): string {
  const kept = preflight.kept_thread_count;
  if (kept === 0) return 'Nothing to archive.';
  const stay = kept === 1 ? '1 thread stays open' : `${kept} threads stay open`;
  return `Nothing to archive. ${stay}: ${keptSummary(preflight.kept)}.`;
}

/** The confirmed families that changed and stayed, counted as the confirm
 *  counts: each family once under its slug, its other threads beside it. */
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

/** What the toast says once the archive answered. `changed` counts, by slug,
 *  the confirmed threads that changed since the confirm and stayed. */
export function archiveAllResultMessage(archived: number, changed: Record<string, number>): string {
  const done = `Archived ${plural(archived, 'thread', 'threads')}.`;
  const total = Object.values(changed).reduce((a, b) => a + b, 0);
  return total === 0 ? done : `${done} ${total} changed since you confirmed and stayed: ${keptSummary(changed)}.`;
}

export async function handleArchiveAll(): Promise<void> {
  if (pressInFlight) return;
  pressInFlight = true;
  try {
    await runArchiveAll();
  } finally {
    pressInFlight = false;
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
  const ok = await showConfirm(confirmation.message, 'Archive', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'default',
  });
  if (!ok) return;

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

/** Undo keeps its own button: a tap on the toast to read or dismiss it must
 *  not bring the whole batch back. */
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
