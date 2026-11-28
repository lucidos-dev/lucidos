
import { signal } from '@preact/signals';
import { clearDraft, getDraft, setDraft, type ComposeDraft } from './composeDrafts';
import { getComposeSelectionOverride, patchComposeSelection, setComposeSelectionFromServer, type ComposeSelectionOverride } from './composeSelections';
import { sameHashes, type ParsedComposeReport } from './composeReport';
import { restoreAdmissionProvisional } from '../utils/restoreAdmission';

export type { ParsedComposeReport, ComposeReportWire } from './composeReport';
export { parseComposeMode, parseComposeReport, sameHashes } from './composeReport';

const serverCompose = new Map<string, ParsedComposeReport>();
const composeEpoch = new Map<string, number>();
const editSeq = new Map<string, number>();
const ackedSeq = new Map<string, number>();

export const serverComposeVersion = signal(0);

export function serverComposeOf(threadId: string): ParsedComposeReport | undefined {
  return serverCompose.get(threadId);
}

export function knownComposeEpoch(threadId: string): number | undefined {
  return composeEpoch.get(threadId);
}

function noteComposeEpoch(threadId: string, epoch: number | null): void {
  if (epoch === null) return;
  const known = composeEpoch.get(threadId);
  if (known === undefined || epoch > known) composeEpoch.set(threadId, epoch);
}

export function noteLocalComposeEdit(threadId: string): number {
  const next = (editSeq.get(threadId) ?? 0) + 1;
  editSeq.set(threadId, next);
  return next;
}

export function composeEditSeq(threadId: string): number {
  return editSeq.get(threadId) ?? 0;
}

export function isComposeIntentPending(threadId: string): boolean {
  return composeEditSeq(threadId) > (ackedSeq.get(threadId) ?? 0);
}

type DraftBatch = Map<string, ComposeDraft | null>;

export function acceptComposeReport(threadId: string, report: ParsedComposeReport, batch?: DraftBatch): boolean {
  if (restoreAdmissionProvisional()) return false;
  noteComposeEpoch(threadId, report.epoch);
  const held = serverCompose.get(threadId);
  const heldRevision = held?.revision ?? null;
  if (report.revision !== null && heldRevision !== null && report.revision <= heldRevision) return false;
  if (isComposeIntentPending(threadId)) {
    const additions = (report.restorations ?? []).filter(r => r.epoch > (held?.epoch ?? -1));
    if (additions.length > 0) {
      const local = getDraft(threadId);
      let text = local.text;
      const images = [...local.image_hashes];
      for (const addition of additions) {
        if (addition.text) text = text.trim() ? `${text}\n\n${addition.text}` : addition.text;
        for (const hash of addition.image_hashes) if (!images.includes(hash)) images.push(hash);
      }
      setDraft(threadId, { ...local, text, image_hashes: images });
      noteLocalComposeEdit(threadId);
    }
  }
  serverCompose.set(threadId, {
    ...report,
    revision: report.revision ?? heldRevision,
    epoch: report.epoch ?? held?.epoch ?? null,
    imageHashes: [...report.imageHashes],
  });
  serverComposeVersion.value = serverComposeVersion.peek() + 1;
  if (!isComposeIntentPending(threadId)) {
    showServerCompose(threadId, batch);
  } else if (report.selection?.codingAgent && !getComposeSelectionOverride(threadId).codingAgent) {
    patchComposeSelection(threadId, { codingAgent: report.selection.codingAgent });
  }
  return true;
}

export function acknowledgeComposeEdits(threadId: string, seq: number, report: ParsedComposeReport): void {
  if (!editSeq.has(threadId)) return;
  ackedSeq.set(threadId, Math.max(ackedSeq.get(threadId) ?? 0, seq));
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

export function _recordServerComposeForTesting(threadId: string, text: string, imageHashes: string[]): void {
  const revision = (serverCompose.get(threadId)?.revision ?? 0) + 1;
  serverCompose.set(threadId, { revision, epoch: null, text, imageHashes: [...imageHashes], mode: null, selection: null });
  serverComposeVersion.value = serverComposeVersion.peek() + 1;
}
