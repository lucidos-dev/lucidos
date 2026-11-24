
import type { PongAnswer } from '../eventStream';
import { PRESENCE_OUTCOMES_BY_STRENGTH } from '../generated/engine-constants';

export const PONG_COLLECT_MS = 300;

export type FromPort =
  | {
      t: 'hello';
      pongs: boolean;
      streamUrl: string;
      pongUrl: string;
    }
  | { t: 'pong'; notificationId: string; answer: PongAnswer; sourceGeneration: string | null }
  | { t: 'check' }
  | { t: 'restart' }
  | { t: 'bye' };

export type ToPort =
  | { t: 'introduce' }
  | { t: 'checked' }
  | { t: 'frame'; data: string }
  | { t: 'open'; lateJoin?: boolean }
  | { t: 'error'; silentMs?: number };

export function aggregatePongAnswers(answers: PongAnswer[]): PongAnswer | null {
  if (answers.length === 0) return null;
  const outcome =
    PRESENCE_OUTCOMES_BY_STRENGTH.find((o) => answers.some((a) => a.outcome === o)) ?? 'not_shown';
  return { device_id: answers[0].device_id, outcome };
}
