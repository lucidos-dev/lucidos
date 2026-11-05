/** What the engine holds as each thread's draft, and the edits this device has
 *  made that the engine has not yet acknowledged.
 *
 *  The draft follows the server's *compose revision* (ADR 0437,
 *  `docs/plans/2026-10-12-compose-draft-follows-a-server-revision.md`). Every
 *  *compose report* is ordered by its revision alone. A local edit is a
 *  *pending intent* until a write covering it is answered. While one is
 *  pending the visible draft is the local one. Otherwise it is the newest
 *  report, so a peer write that landed after ours shows. */

import { signal } from '@preact/signals';
import { clearDraft, getDraft, setDraft, type ComposeDraft } from './composeDrafts';
import { getComposeSelectionOverride, setComposeSelectionFromServer, type ComposeSelectionOverride } from './composeSelections';
import { sameHashes, type ComposeReport } from './composeReport';

export type { ComposeReport, ComposeReportWire } from './composeReport';
export { parseComposeMode, parseComposeReport, sameHashes } from './composeReport';

const serverCompose = new Map<string, ComposeReport>();
/** The newest *compose epoch* heard per thread. Monotonic on its own, apart
 *  from `serverCompose`, because a `412` reports it with the write unapplied. */
const composeEpoch = new Map<string, number>();
const editSeq = new Map<string, number>();
const ackedSeq = new Map<string, number>();

/** Bumped whenever `serverCompose` moves, so a subscriber learns the engine's
 *  draft changed. The pending-upload store releases a landed image on it. */
export const serverComposeVersion = signal(0);

/** The newest report this device holds for the thread, or `undefined` when it
 *  has heard none. */
export function serverComposeOf(threadId: string): ComposeReport | undefined {
  return serverCompose.get(threadId);
}

/** The epoch a compose write is fenced against, or `undefined` for "never
 *  heard", which the engine accepts unfenced. */
export function knownComposeEpoch(threadId: string): number | undefined {
  return composeEpoch.get(threadId);
}

function noteComposeEpoch(threadId: string, epoch: number | null): void {
  if (epoch === null) return;
  const known = composeEpoch.get(threadId);
  if (known === undefined || epoch > known) composeEpoch.set(threadId, epoch);
}

/** Record a local compose mutation, and return the edit counter a write that
 *  reads the draft from now on covers. */
export function noteLocalComposeEdit(threadId: string): number {
  const next = (editSeq.get(threadId) ?? 0) + 1;
  editSeq.set(threadId, next);
  return next;
}

/** The edit counter a write reading the draft right now covers. */
export function composeEditSeq(threadId: string): number {
  return editSeq.get(threadId) ?? 0;
}

/** Whether this device holds a compose edit the engine has not acknowledged. */
export function isComposeIntentPending(threadId: string): boolean {
  return composeEditSeq(threadId) > (ackedSeq.get(threadId) ?? 0);
}

type DraftBatch = Map<string, ComposeDraft | null>;

/** Record a report the engine made, and show it unless an edit is pending.
 *  Returns false when it was at or below the revision already held, and so
 *  changed nothing. `batch` collects the visible write for a bulk load. */
export function acceptComposeReport(threadId: string, report: ComposeReport, batch?: DraftBatch): boolean {
  noteComposeEpoch(threadId, report.epoch);
  const held = serverCompose.get(threadId);
  const heldRevision = held?.revision ?? null;
  if (report.revision !== null && heldRevision !== null && report.revision <= heldRevision) return false;
  serverCompose.set(threadId, {
    ...report,
    revision: report.revision ?? heldRevision,
    imageHashes: [...report.imageHashes],
  });
  serverComposeVersion.value = serverComposeVersion.peek() + 1;
  if (!isComposeIntentPending(threadId)) showServerCompose(threadId, batch);
  return true;
}

/** A write covering edits up to `seq` was answered, with the report it left.
 *  An answer for a thread this module has forgotten (discarded or deleted
 *  meanwhile) changes nothing. */
export function acknowledgeComposeEdits(threadId: string, seq: number, report: ComposeReport): void {
  if (!editSeq.has(threadId)) return;
  ackedSeq.set(threadId, Math.max(ackedSeq.get(threadId) ?? 0, seq));
  // A newer report shows itself. An older one leaves the newest server state
  // to show, now that nothing is pending.
  if (!acceptComposeReport(threadId, report) && !isComposeIntentPending(threadId)) showServerCompose(threadId);
}

function sameSelection(a: ComposeSelectionOverride, b: ComposeSelectionOverride | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b ?? {});
}

function showServerCompose(threadId: string, batch?: DraftBatch): void {
  const report = serverCompose.get(threadId);
  if (!report) return;
  if (!sameSelection(getComposeSelectionOverride(threadId), report.selection)) {
    setComposeSelectionFromServer(threadId, report.selection);
  }
  const empty = report.text === '' && report.imageHashes.length === 0 && report.mode === null;
  if (batch) {
    batch.set(threadId, empty ? null : { text: report.text, image_hashes: [...report.imageHashes], mode: report.mode });
    return;
  }
  const shown = getDraft(threadId);
  if (shown.text === report.text && shown.mode === report.mode && sameHashes(shown.image_hashes, report.imageHashes)) return;
  if (empty) clearDraft(threadId);
  else setDraft(threadId, { text: report.text, image_hashes: [...report.imageHashes], mode: report.mode });
}

/** Drop every record for a thread that will never be fetched again. */
export function forgetComposeSync(threadId: string): void {
  serverCompose.delete(threadId);
  composeEpoch.delete(threadId);
  editSeq.delete(threadId);
  ackedSeq.delete(threadId);
}

export function _resetComposeSyncForTesting(): void {
  serverCompose.clear();
  composeEpoch.clear();
  editSeq.clear();
  ackedSeq.clear();
}

/** Test-only: record what the engine holds without showing it, as a test that
 *  models only the server side of a draft needs. */
export function _recordServerComposeForTesting(threadId: string, text: string, imageHashes: string[]): void {
  const revision = (serverCompose.get(threadId)?.revision ?? 0) + 1;
  serverCompose.set(threadId, { revision, epoch: null, text, imageHashes: [...imageHashes], mode: null, selection: null });
  serverComposeVersion.value = serverComposeVersion.peek() + 1;
}
