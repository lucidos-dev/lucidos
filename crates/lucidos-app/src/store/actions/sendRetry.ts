import { isHookInputOffer } from '../../api/client/eventHooks';
import { ApiError, isTransientFetchError } from '../../api/client';
import { postClientLog } from '../../utils/clientLog';
import { errorDetail } from '../../utils/errorDetail';
import { onPageWake } from '../../utils/pageVisit';
import type { UnsentMessage } from '../unsentMessages';

export const SEND_RETRY_BACKOFF_MS = [1_000, 3_000, 8_000] as const;

export const SEND_RETRY_DEADLINE_MS = 30_000;

export type SendPath = 'message' | 'answer' | 'side-question';

export type QuietRetryResult<T> =
  | { kind: 'done'; value: T; attempts: number }
  | { kind: 'landed'; attempts: number }
  | { kind: 'gave-up'; error: unknown; attempts: number }
  | { kind: 'refused'; error: unknown; attempts: number };

interface QuietRetryOptions {
  path: SendPath;
  retryable?: (error: unknown, attemptMs: number) => boolean;
  landed?: () => boolean;
  beforeRetry?: (error: unknown) => void;
}

export interface SendFailure {
  attempts: number;
  reason: string;
}

export function sendFailureOf(result: { attempts: number; error: unknown }): SendFailure {
  return { attempts: result.attempts, reason: errorDetail(result.error) };
}

export function inputRefusalOf(error: unknown): UnsentMessage['refusal'] {
  if (!(error instanceof ApiError) || error.httpCode !== 409 || typeof error.body !== 'object' || error.body === null || !('reason' in error.body)) return undefined;
  const reason = error.body.reason;
  if (reason === 'hook_needs_input' && 'hook_input' in error.body && isHookInputOffer(error.body.hook_input)) {
    return { reason, message: error.reason, input: error.body.hook_input };
  }
  if (reason !== 'overtaken' && reason !== 'compose_snapshot_stale' && reason !== 'compose_fence_required'
    && reason !== 'hook_input_overtaken') return undefined;
  return { reason, message: error.reason };
}

export async function withQuietRetries<T>(
  attempt: () => Promise<T>,
  options: QuietRetryOptions,
): Promise<QuietRetryResult<T>> {
  const retryable = options.retryable ?? isTransientFetchError;
  const firstStarted = Date.now();
  let lastError: unknown;
  const settle = (result: QuietRetryResult<T>): QuietRetryResult<T> => {
    if (result.attempts > 1) logRetriedSend(options.path, result, lastError);
    return result;
  };
  for (let attempts = 1; ; attempts++) {
    const started = Date.now();
    try {
      return settle({ kind: 'done', value: await attempt(), attempts });
    } catch (error) {
      lastError = error;
      if (!retryable(error, Date.now() - started)) return settle({ kind: 'refused', error, attempts });
      if (options.landed?.()) return settle({ kind: 'landed', attempts });
      const delay = SEND_RETRY_BACKOFF_MS[attempts - 1];
      if (delay === undefined || Date.now() - firstStarted + delay > SEND_RETRY_DEADLINE_MS) {
        return settle({ kind: 'gave-up', error, attempts });
      }
      options.beforeRetry?.(error);
      await waitBeforeRetry(delay);
      if (options.landed?.()) return settle({ kind: 'landed', attempts });
      if (Date.now() - firstStarted > SEND_RETRY_DEADLINE_MS) return settle({ kind: 'gave-up', error, attempts });
    }
  }
}

function waitBeforeRetry(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const win = typeof window === 'undefined' ? undefined : window;
    const done = () => {
      clearTimeout(timer);
      win?.removeEventListener?.('online', done);
      stopWatchingWake();
      resolve();
    };
    const timer = setTimeout(done, ms);
    win?.addEventListener?.('online', done);
    const stopWatchingWake = onPageWake(done);
  });
}

function logRetriedSend(path: SendPath, result: QuietRetryResult<unknown>, lastError: unknown): void {
  postClientLog('chat', 'send_retried', {
    path,
    attempts: result.attempts,
    outcome: result.kind,
    error: errorName(lastError),
  });
}

function errorName(error: unknown): string {
  return error instanceof Error ? error.name : typeof error;
}
