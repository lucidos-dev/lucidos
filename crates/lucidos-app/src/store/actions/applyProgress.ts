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
import { changeHeadline } from '../changeHeadline';
import { focusThreadOrBootstrap } from './threads';

export const APPLY_PHASE_LABEL: Record<ApplyPhase, string> = {
  merging: 'Merging',
  'resolving-conflict': 'Resolving merge conflict',
  hardening: 'Hardening',
};

const UNKNOWN_PHASE_LABEL = 'Applying changes';

const SECS_PER_MIN = 60;

export function formatDuration(secs: number): string {
  const minutes = Math.floor(secs / SECS_PER_MIN);
  if (minutes < 1) return 'under a minute';
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

function typicalSecs(phase: ApplyPhase, estimates: ApplyEstimates): number | null {
  if (phase === 'hardening') return estimates.hardening?.typical_secs ?? null;
  if (phase === 'resolving-conflict') return estimates.resolving_conflict?.typical_secs ?? null;
  return null;
}

function elapsedSecs(reading: ApplyPhaseReading, nowMs: number): number | null {
  const started = reading.startedAt ? Date.parse(reading.startedAt) : NaN;
  return Number.isFinite(started) ? Math.max(0, (nowMs - started) / 1000) : null;
}

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
  if (change.needs_hardening) return { phase: 'hardening', eventId: null, startedAt };
  return { phase: 'merging', eventId: null, startedAt: null };
}

function phasesAhead(change: Change, phase: ApplyPhase): ApplyPhase[] | null {
  if (phase === 'resolving-conflict') return ['resolving-conflict'];
  const prediction = change.predicted_conflict;
  if (prediction !== 'conflict' && prediction !== 'clean') return null;
  const conflict: ApplyPhase[] = prediction === 'conflict' ? ['resolving-conflict'] : [];
  return phase === 'hardening' ? ['hardening', ...conflict] : conflict;
}

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

export function batchTimeLeftLabel(secondsLeft: number | null): string | null {
  return secondsLeft !== null && secondsLeft >= SECS_PER_MIN
    ? `about ${formatDuration(secondsLeft)} until all are applied`
    : null;
}

function unresolvedMembers(batch: ApplyAllBatch, pendingIds: ReadonlySet<string> | null): string[] {
  const resolved = new Set(batch.resolvedChangeIds);
  return batch.changeIds.filter((id) => !resolved.has(id) && (pendingIds === null || pendingIds.has(id)));
}

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

export function openApplyPhase(
  threadId: string,
  changeId: string | undefined,
  reading: ApplyPhaseReading | null | undefined,
): void {
  focusThreadOrBootstrap(threadId, reading?.eventId
    ? { targetEventId: reading.eventId }
    : { targetChangeId: changeId ?? null });
}

export function batchSummary(applied: number, total: number): { message: string; type: 'success' | 'info' } {
  const changes = (n: number) => `${n} change${n === 1 ? '' : 's'}`;
  if (applied === total) return { message: `Applied ${changes(total)}`, type: 'success' };
  const missed = total - applied;
  return {
    message: `Applied ${applied} of ${changes(total)}. ${missed} did not apply.`,
    type: 'info',
  };
}

export function setApplyPhase(threadId: string, reading: ApplyPhaseReading): void {
  const next = new Map(applyPhases.value);
  next.set(threadId, reading);
  applyPhases.value = next;
}

export function clearApplyPhase(threadId: string): ApplyPhaseReading | undefined {
  const reading = applyPhases.value.get(threadId);
  if (!reading) return undefined;
  const next = new Map(applyPhases.value);
  next.delete(threadId);
  applyPhases.value = next;
  return reading;
}

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
    const changeId = previousPending.find((c) => c.thread_id === threadId)?.id;
    const appliedChange = changeId ? applied.find((c) => c.id === changeId) : undefined;
    if (!appliedChange || isBatchMember(applyAllBatch.value, appliedChange.id)) continue;
    showToast(changeToastMessage('Applied', threadId, changeHeadline(appliedChange)), 'success', {
      key: `applying-${threadId}`,
      onClick: () => openApplyPhase(threadId, appliedChange.id, reading),
      autoDismissMs: TOAST_AUTO_DISMISS_MS,
    });
  }
}
