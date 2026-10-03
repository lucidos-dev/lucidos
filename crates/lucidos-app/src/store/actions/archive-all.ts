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
  type ArchiveAllPreflight,
} from '../../api/threads';
import { showConfirm, showToast } from '../store';
import type { ToastAction } from '../types';
import { errorDetail } from '../../utils/errorDetail';

/** How long the Undo stays on screen. */
const UNDO_MS = 10_000;

/** The most ids one request carries: `MAX_IDS` in `api/threads/archive_all.rs`. */
export const MAX_IDS_PER_REQUEST = 2_000;

/** Split `ids` into requests the engine accepts. */
export function idBatches(ids: string[]): string[][] {
  const batches: string[][] = [];
  for (let i = 0; i < ids.length; i += MAX_IDS_PER_REQUEST) {
    batches.push(ids.slice(i, i + MAX_IDS_PER_REQUEST));
  }
  return batches;
}

/** A press still running, so a second press cannot start a parallel one. */
let pressInFlight = false;

/** Each kept reason as a count phrase, singular then plural. */
const KEPT_PHRASES: Record<string, [string, string]> = {
  question: ['open question', 'open questions'],
  pending_change: ['unapplied change', 'unapplied changes'],
  unproposed_work: ['with unproposed work', 'with unproposed work'],
  draft: ['unsent draft', 'unsent drafts'],
  failed_run: ['failed run', 'failed runs'],
  busy: ['still working', 'still working'],
};

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The confirm, or `null` when nothing in Current is safe to archive. Pure,
 *  so every line of the copy is testable without a dialog. */
export function archiveAllConfirmation(
  preflight: ArchiveAllPreflight,
): { title: string; message: string } | null {
  if (preflight.safe.length === 0) return null;
  const title = `Archive ${plural(preflight.safe.length, 'thread', 'threads')}?`;
  if (preflight.kept_count === 0) {
    return { title, message: 'Everything in Current goes to Archive. You can undo right after.' };
  }
  const reasons = Object.entries(preflight.kept)
    .filter(([, n]) => n > 0)
    .map(([slug, n]) => {
      const [one, many] = KEPT_PHRASES[slug] ?? ['other', 'other'];
      return plural(n, one, many);
    })
    .join(', ');
  const needs = preflight.kept_count === 1 ? '1 needs you and stays' : `${preflight.kept_count} need you and stay`;
  return { title, message: `${needs} in Current: ${reasons}. You can undo right after.` };
}

/** What the toast says once the archive answered. */
export function archiveAllResultMessage(archived: number, changed: number): string {
  const done = `Archived ${plural(archived, 'thread', 'threads')}.`;
  return changed === 0 ? done : `${done} ${changed} changed since you confirmed and stayed.`;
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
    showToast(
      preflight.kept_count === 0
        ? 'Nothing to archive.'
        : `Nothing to archive. ${plural(preflight.kept_count, 'thread', 'threads')} in Current need you.`,
      'info',
    );
    return;
  }
  const ok = await showConfirm(confirmation.message, 'Archive', {
    title: confirmation.title,
    cancelLabel: 'Cancel',
    variant: 'default',
  });
  if (!ok) return;

  const batch: string[] = [];
  let changed = 0;
  try {
    for (const ids of idBatches(preflight.safe.map((t) => t.thread_id))) {
      const result = await archiveAll(ids);
      batch.push(...result.archived);
      changed += result.kept.length;
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
  showToast(archiveAllResultMessage(batch.length, changed), 'success', {
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
