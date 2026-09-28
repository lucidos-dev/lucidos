import type { ApplyEstimates, Change } from '../../api/client';
import {
  applyAllBatch,
  applyingNowThreadIds,
  applyPhases,
  showToast,
  threadMap,
  TOAST_AUTO_DISMISS_MS,
  type ApplyAllBatch,
  type ApplyPhase,
  type ApplyPhaseReading,
} from '../store';
import { PENDING_TITLE_PLACEHOLDER } from '../thread-events';
import { changeToastMessage } from './changeToast';
import { focusThread } from './threads';

/** What an apply is doing, as the activity group names it. */
export const APPLY_PHASE_LABEL: Record<ApplyPhase, string> = {
  merging: 'Merging',
  'resolving-conflict': 'Resolving merge conflict',
  hardening: 'Hardening',
};

/** The label when nothing says what the apply is doing yet. */
const UNKNOWN_PHASE_LABEL = 'Applying changes';

const SECS_PER_MIN = 60;

/** A duration as the activity group says it: minutes, then hours. */
export function formatDuration(secs: number): string {
  const minutes = Math.floor(secs / SECS_PER_MIN);
  if (minutes < 1) return 'under a minute';
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

/** The typical duration of a slow phase, or `null` for a plain merge and for
 *  a phase with too few past runs to estimate. */
function typicalSecs(phase: ApplyPhase, estimates: ApplyEstimates): number | null {
  if (phase === 'hardening') return estimates.hardening?.typical_secs ?? null;
  if (phase === 'resolving-conflict') return estimates.resolving_conflict?.typical_secs ?? null;
  return null;
}

/** How long the phase has run, or `null` when nothing says when it began. */
function elapsedSecs(reading: ApplyPhaseReading, nowMs: number): number | null {
  const started = reading.startedAt ? Date.parse(reading.startedAt) : NaN;
  return Number.isFinite(started) ? Math.max(0, (nowMs - started) / 1000) : null;
}

/** What the apply is doing, and for a slow phase how long it has run and
 *  usually runs: "Resolving merge conflict · 6 min, usually ~18 min". */
export function phaseLabel(
  reading: ApplyPhaseReading | null,
  estimates: ApplyEstimates,
  nowMs: number,
): string {
  if (!reading) return UNKNOWN_PHASE_LABEL;
  const label = APPLY_PHASE_LABEL[reading.phase];
  const elapsed = reading.phase === 'merging' ? null : elapsedSecs(reading, nowMs);
  if (elapsed === null) return label;
  const typical = typicalSecs(reading.phase, estimates);
  const usually = typical === null ? '' : `, usually ~${formatDuration(typical)}`;
  return `${label} · ${formatDuration(elapsed)}${usually}`;
}

/** What the apply of `change` on `threadId` is doing, or `null` when nothing
 *  says. A phase event this page saw wins. Otherwise the row answers, which is
 *  all a reloaded page has: an unhardened change hardens before it merges. */
export function applyPhaseOf(
  threadId: string | null,
  change: Change | undefined,
  phases: ReadonlyMap<string, ApplyPhaseReading>,
): ApplyPhaseReading | null {
  const seen = threadId ? phases.get(threadId) : undefined;
  if (seen) return seen;
  if (!change) return null;
  const startedAt = change.apply_phase_started_at ?? null;
  if (change.resolving_conflict) return { phase: 'resolving-conflict', eventId: null, startedAt };
  if (!change.hardened) return { phase: 'hardening', eventId: null, startedAt };
  return { phase: 'merging', eventId: null, startedAt: null };
}

/** The slow phases an apply of `change` has still to run, from `phase` on. A
 *  hardening comes before the merge, and a predicted conflict comes after it.
 *  `null` when the merge is still ahead and git could not predict it. */
function phasesAhead(change: Change, phase: ApplyPhase): ApplyPhase[] | null {
  if (phase === 'resolving-conflict') return ['resolving-conflict'];
  const prediction = change.predicted_conflict;
  if (prediction !== 'conflict' && prediction !== 'clean') return null;
  const conflict: ApplyPhase[] = prediction === 'conflict' ? ['resolving-conflict'] : [];
  return phase === 'hardening' ? ['hardening', ...conflict] : conflict;
}

/** Roughly how long the batch has left, in seconds, or `null` when a term of
 *  the sum is unknown. A partial sum would promise less time than the batch
 *  needs.
 *
 *  Hardening runs one member at a time, so it adds up. A conflict resolution
 *  runs beside the queue (ADR 0314), so it ends at its own time: when the
 *  queue reached it, plus its typical length. A phase in flight counts its
 *  typical time minus what has run, never below a minute. */
export function batchSecondsLeft(
  batch: ApplyAllBatch,
  pending: readonly Change[] | null,
  phases: ReadonlyMap<string, ApplyPhaseReading>,
  estimates: ApplyEstimates,
  nowMs: number,
): number | null {
  if (!pending) return null;
  const pendingIds = new Set(pending.map((c) => c.id));
  const inFlight = batchMemberInFlight(batch, pendingIds);
  if (!inFlight) return null;
  const resolving = new Set(batch.resolvingChangeIds);
  let queueClock = 0;
  let lastEnd = 0;
  for (const changeId of unresolvedMembers(batch, pendingIds)) {
    const change = pending.find((c) => c.id === changeId);
    if (!change) continue;
    const active = changeId === inFlight.changeId || resolving.has(changeId);
    const reading = active
      ? applyPhaseOf(change.thread_id, change, phases)
      : applyPhaseOf(null, change, new Map());
    const ahead = reading ? phasesAhead(change, reading.phase) : null;
    if (!reading || !ahead) return null;
    for (const [i, phase] of ahead.entries()) {
      const typical = typicalSecs(phase, estimates);
      if (typical === null) return null;
      const running = active && i === 0 && phase === reading.phase;
      const secs = running
        ? Math.max(SECS_PER_MIN, typical - (elapsedSecs(reading, nowMs) ?? 0))
        : typical;
      if (phase === 'hardening') {
        queueClock += secs;
      } else {
        lastEnd = Math.max(lastEnd, (running ? 0 : queueClock) + secs);
      }
    }
  }
  return Math.max(queueClock, lastEnd);
}

/** "about 20 min until all are applied", or `null` when the batch cannot
 *  say, or has under a minute to go. It names the whole batch, so it never
 *  reads as the time of the phase in flight. */
export function batchTimeLeftLabel(secondsLeft: number | null): string | null {
  return secondsLeft !== null && secondsLeft >= SECS_PER_MIN
    ? `about ${formatDuration(secondsLeft)} until all are applied`
    : null;
}

/** Members still to resolve, in batch order. A member no longer pending
 *  resolved too, even with no event this page saw: a discard, or an apply
 *  that ended with no thread event. `pendingIds` is `null` while the pending
 *  list is not loaded. */
function unresolvedMembers(batch: ApplyAllBatch, pendingIds: ReadonlySet<string> | null): string[] {
  const resolved = new Set(batch.resolvedChangeIds);
  return batch.changeIds.filter((id) => !resolved.has(id) && (pendingIds === null || pendingIds.has(id)));
}

/** The member the Apply All row names: the one the engine is applying.
 *  Before the engine says, it is the first unresolved member that is not
 *  parked, as a batch applies in order. When only parked members are left,
 *  it names the first of them.
 *
 *  `position` counts the members already done or parked, then this one.
 *  `resolved` counts the done ones alone, for the progress bar. */
export function batchMemberInFlight(
  batch: ApplyAllBatch,
  pendingIds: ReadonlySet<string> | null,
): { changeId: string; position: number; resolved: number; total: number } | null {
  const unresolved = unresolvedMembers(batch, pendingIds);
  if (unresolved.length === 0) return null;
  const parked = new Set(batch.resolvingChangeIds);
  const changeId = unresolved.find((id) => batch.applyingChangeIds.includes(id))
    ?? unresolved.find((id) => !parked.has(id))
    ?? unresolved[0];
  const total = batch.changeIds.length;
  const resolved = total - unresolved.length;
  const othersParked = unresolved.filter((id) => id !== changeId && parked.has(id)).length;
  return { changeId, position: resolved + othersParked + 1, resolved, total };
}

/** Parked members the Apply All row does not name. Each gets its own row. */
export function batchMembersParkedBeside(
  batch: ApplyAllBatch,
  pendingIds: ReadonlySet<string> | null,
): string[] {
  const named = batchMemberInFlight(batch, pendingIds)?.changeId;
  const parked = new Set(batch.resolvingChangeIds);
  return unresolvedMembers(batch, pendingIds).filter((id) => id !== named && parked.has(id));
}

export function isBatchMember(batch: ApplyAllBatch | null, changeId: string | undefined): boolean {
  return !!batch && !!changeId && batch.changeIds.includes(changeId);
}

export function threadTitle(threadId: string | null, change: Change | undefined): string | null {
  const title = threadId ? threadMap.value.get(threadId)?.meta.title : undefined;
  if (title && title !== PENDING_TITLE_PLACEHOLDER) return title;
  return change?.thread_title ?? null;
}

/** Where an apply's thread link lands: the event that started the phase, or
 *  the change's own turn when this page never saw one. */
export function openApplyPhase(
  threadId: string,
  changeId: string | undefined,
  reading: ApplyPhaseReading | null | undefined,
): void {
  focusThread(threadId, reading?.eventId
    ? { targetEventId: reading.eventId }
    : { targetChangeId: changeId ?? null });
}

/** The summary an Apply All ends with. */
export function batchSummary(applied: number, total: number): { message: string; type: 'success' | 'info' } {
  const changes = (n: number) => `${n} change${n === 1 ? '' : 's'}`;
  if (applied === total) return { message: `Applied ${changes(total)}`, type: 'success' };
  const missed = total - applied;
  return {
    message: `Applied ${applied} of ${changes(total)}. ${missed} did not apply.`,
    type: 'info',
  };
}

/** Record what the apply on `threadId` is doing now. */
export function setApplyPhase(threadId: string, reading: ApplyPhaseReading): void {
  const next = new Map(applyPhases.value);
  next.set(threadId, reading);
  applyPhases.value = next;
}

/** Forget the thread's phase: its change resolved. Returns what it was, so
 *  the result toast can still land on the event that started it. */
export function clearApplyPhase(threadId: string): ApplyPhaseReading | undefined {
  const reading = applyPhases.value.get(threadId);
  if (!reading) return undefined;
  const next = new Map(applyPhases.value);
  next.delete(threadId);
  applyPhases.value = next;
  return reading;
}

/** Settle apply progress against the fetched lists, for a terminal event a
 *  suspended page or a dropped stream never saw. Runs on startup and resume.
 *
 *  A phase stays only while the engine says its apply still runs: the change
 *  is pending, and its thread is settling or resolving the conflict. A failed
 *  apply leaves the change pending on an idle thread, so its phase goes too.
 *  When the change the page last saw pending has landed, the result the stream
 *  dropped is reported. Apply Now's own threads are `reconcileApplyingNow`'s. */
export function reconcileApplyProgress(
  previousPending: readonly Change[],
  pending: readonly Change[],
  applied: readonly Change[],
): void {
  const running = new Set(
    pending
      .filter((c) => c.thread_settling || c.resolving_conflict)
      .map((c) => c.thread_id)
      .filter((id): id is string => !!id),
  );
  const settled = [...applyPhases.value].filter(([threadId]) => !running.has(threadId));
  if (settled.length === 0) return;
  applyPhases.value = new Map([...applyPhases.value].filter(([threadId]) => running.has(threadId)));

  for (const [threadId, reading] of settled) {
    if (applyingNowThreadIds.value.has(threadId)) continue;
    // Match the change this apply was for, never an older one of the thread.
    const changeId = previousPending.find((c) => c.thread_id === threadId)?.id;
    const appliedChange = changeId ? applied.find((c) => c.id === changeId) : undefined;
    // A batch member's result is the batch summary's to report.
    if (!appliedChange || isBatchMember(applyAllBatch.value, appliedChange.id)) continue;
    showToast(changeToastMessage('Applied', threadId, appliedChange.description), 'success', {
      key: `applying-${threadId}`,
      onClick: () => openApplyPhase(threadId, appliedChange.id, reading),
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
  }
}
