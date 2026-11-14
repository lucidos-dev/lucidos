/**
 * The *main check* on the client (ADR 0464): what the `MainCheck*` frames do
 * to the `mainCheck` signal. The maintainer's Close lives in
 * `laneControls.ts`, which only lazy surfaces load (ADR 0454).
 *
 * The engine owns the check. `GET /api/v1/changes` and every `ChangesUpdated`
 * frame carry its snapshot whole. The frames move it between those, so a
 * check shows the moment it starts or ends.
 */

import type { MainCheck } from '../../api/client';
import { IDLE_MAIN_CHECK, mainCheck, mainCheckClosing } from '../store';

/** The main check frames, by name. */
export type MainCheckFrame =
  | 'MainCheckStarted'
  | 'MainCheckRunLinked'
  | 'MainCheckPassed'
  | 'MainCheckFailed'
  | 'MainCheckAbandoned'
  | 'MainCheckRedClosed';

const text = (value: unknown): string | undefined =>
  typeof value === 'string' && value !== '' ? value : undefined;

/** The check after `frame`. A frame about another check leaves it alone,
 *  except a start, which is always the newest check. */
export function mainCheckAfter(current: MainCheck, frame: MainCheckFrame, data: Record<string, unknown>): MainCheck {
  const checkId = text(data.check_id);
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
    case 'MainCheckRedClosed':
      return current.state === 'red' ? IDLE_MAIN_CHECK : current;
  }
}

/** Take the engine's snapshot. A Close pressed for a red that has since
 *  closed stops reading as closing. An older engine sends none. */
export function setMainCheckFromServer(check: MainCheck | undefined): void {
  const next = check ?? IDLE_MAIN_CHECK;
  mainCheck.value = next;
  if (mainCheckClosing.value && !(next.state === 'red' && next.check_id === mainCheckClosing.value)) {
    mainCheckClosing.value = null;
  }
}

/** Apply a main check frame. */
export function handleMainCheckFrame(frame: MainCheckFrame, data: Record<string, unknown>): void {
  setMainCheckFromServer(mainCheckAfter(mainCheck.value, frame, data));
}
