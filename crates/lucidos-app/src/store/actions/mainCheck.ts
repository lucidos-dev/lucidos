
import type { MainCheck } from '../../api/client';
import { IDLE_MAIN_CHECK, mainCheck, mainCheckClosing } from '../store';

export type MainCheckFrame =
  | 'MainCheckStarted'
  | 'MainCheckRunLinked'
  | 'MainCheckPassed'
  | 'MainCheckFailed'
  | 'MainCheckAbandoned'
  | 'MainCheckSuperseded'
  | 'MainCheckRedClosed'
  | 'MainCheckRepairStarted';

const text = (value: unknown): string | undefined =>
  typeof value === 'string' && value !== '' ? value : undefined;

export function mainCheckAfter(current: MainCheck, frame: MainCheckFrame, data: Record<string, unknown>): MainCheck {
  const checkId = text(data.check_id);
  if (frame === 'MainCheckRepairStarted') {
    if (current.repair?.fix_thread_id !== text(data.fix_thread_id)) return current;
    return { ...current, repair: { ...current.repair, state: 'running',
      started_by: data.started_by === 'auto' ? 'auto' : 'fix-now', waiting_on: [] } };
  }
  if (current.repair) return current;
  if (frame === 'MainCheckFailed' && data.superseded_by_fix) return current;
  if (frame === 'MainCheckStarted') {
    return { state: 'running', check_id: checkId, from_sha: text(data.from_sha), to_sha: text(data.to_sha) };
  }
  if (current.check_id !== checkId) return current;
  const range = { check_id: checkId, from_sha: current.from_sha, to_sha: current.to_sha };
  switch (frame) {
    case 'MainCheckRunLinked':
      return current.state === 'running' ? { ...current, run_url: text(data.run_url) } : current;
    case 'MainCheckPassed':
      return { state: 'passed', ...range };
    case 'MainCheckFailed':
      return {
        state: 'red',
        ...range,
        run_url: text(data.run_url),
        fix_thread_id: text(data.fix_thread_id),
        summary: text(data.summary),
      };
    case 'MainCheckAbandoned':
      return IDLE_MAIN_CHECK;
    case 'MainCheckSuperseded':
      return current;
    case 'MainCheckRedClosed':
      return current.state === 'red' ? IDLE_MAIN_CHECK : current;
  }
}

export function setMainCheckFromServer(check: MainCheck | undefined): void {
  const next = check ?? IDLE_MAIN_CHECK;
  mainCheck.value = next;
  if (mainCheckClosing.value && !(next.state === 'red' && next.check_id === mainCheckClosing.value)) {
    mainCheckClosing.value = null;
  }
}

export function handleMainCheckFrame(frame: MainCheckFrame, data: Record<string, unknown>): void {
  setMainCheckFromServer(mainCheckAfter(mainCheck.value, frame, data));
}
