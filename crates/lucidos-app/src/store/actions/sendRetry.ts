import { isTransientFetchError } from '../../api/client';
import { postClientLog } from '../../utils/clientLog';
import { errorDetail } from '../../utils/errorDetail';
import { onPageWake } from '../../utils/pageVisit';

/** The waits before each quiet retry of a send. A dropped connection fails in
 *  under a second, so all three fit in about 13 s. */
export const SEND_RETRY_BACKOFF_MS = [1_000, 3_000, 8_000] as const;

/** No retry starts later than this after the first attempt began. A stalled
 *  POST spends `SUBMIT_CHAT_TIMEOUT_MS` per attempt, so it gets one retry. */
export const SEND_RETRY_DEADLINE_MS = 30_000;

/** Which send path retried, for the breadcrumb. */
export type SendPath = 'message' | 'answer' | 'side-question';

export type QuietRetryResult<T> =
  | { kind: 'done'; value: T; attempts: number }
  /** `landed` reported the send arrived, so no further attempt went out. */
  | { kind: 'landed'; attempts: number }
  /** Every attempt failed transiently, or the deadline came first. */
  | { kind: 'gave-up'; error: unknown; attempts: number }
  /** A failure that is a verdict, which no retry changes. */
  | { kind: 'refused'; error: unknown; attempts: number };

interface QuietRetryOptions {
  path: SendPath;
  /** Whether a failure is worth another attempt. Defaults to
   *  `isTransientFetchError`. `attemptMs` is how long the failed attempt took. */
  retryable?: (error: unknown, attemptMs: number) => boolean;
  /** Asked after each retryable failure: did the send arrive anyway? */
  landed?: () => boolean;
  /** Runs before each wait, for a caller that redraws per attempt. */
  beforeRetry?: (error: unknown) => void;
}

/** What a send's quiet retries ended on, kept for its Not sent details. */
export interface SendFailure {
  attempts: number;
  reason: string;
}

export function sendFailureOf(result: { attempts: number; error: unknown }): SendFailure {
  return { attempts: result.attempts, reason: errorDetail(result.error) };
}

/** The details a Not sent notice opens to. */
export function sendFailureDetail(failure: SendFailure): string {
  const tries = failure.attempts === 1 ? 'try' : 'tries';
  return `Lucidos did not answer after ${failure.attempts} ${tries}. Last error: ${failure.reason}.`;
}

/** Run a send, retrying failures that say nothing about the request, on
 *  `SEND_RETRY_BACKOFF_MS` within `SEND_RETRY_DEADLINE_MS`. Never throws.
 *
 *  The first attempt starts synchronously: a lone send must still go out in
 *  its caller's turn. Every send path posts through this, so a Not sent means
 *  the same thing wherever it shows. */
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
      // A throttled or frozen timer can fire long after its delay.
      if (Date.now() - firstStarted > SEND_RETRY_DEADLINE_MS) return settle({ kind: 'gave-up', error, attempts });
    }
  }
}

/** Resolve after `ms`, or sooner when the browser reports the network back or
 *  the page comes back to the foreground. A send that failed while the page
 *  was away is worth trying again the moment it returns. */
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

/** One breadcrumb per send that needed a retry, so the schedule can be tuned
 *  from what happens on real links. Best-effort telemetry: the send's own
 *  surface reports the outcome, and `postClientLog` never throws. */
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
