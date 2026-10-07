/**
 * Exchange status state machine.
 *
 * Replaces the 4 boolean flags (isProcessing, isPending, wasInterrupted, canceled)
 * with a single enum.
 */

export type ExchangeStatus =
  | 'pending'          // Created, waiting for first SSE event
  | 'queued'           // Waiting for a prior active exchange to finish
  | 'held'             // A callback waiting behind an open question's answer
  | 'streaming'        // SSE events flowing (text/tools)
  | 'coding-agent-working'       // Claude Code actively working
  | 'awaiting-answer'  // CC paused on a question or permission prompt — user's turn
  | 'done'             // Response complete
  | 'interrupted'      // User sent follow-up while streaming
  | 'canceled'         // User explicitly canceled
  | 'error'            // Request failed
  | 'aborted';         // System interrupted (engine restart/crash mid-response)

export const ACTIVE_STATUSES: Set<ExchangeStatus> = new Set(['pending', 'streaming', 'coding-agent-working']);

export function isActive(status: ExchangeStatus): boolean {
  return ACTIVE_STATUSES.has(status);
}

/** Statuses where the response is no longer running AND the agent will not act
 *  on a fresh AskUserQuestion / permission answer landing after this point.
 *  Used by ChatExchange to disable inline question / permission buttons whose
 *  surrounding response was canceled / aborted / failed / superseded. */
export const TERMINATED_STATUSES: Set<ExchangeStatus> = new Set([
  'canceled',
  'aborted',
  'error',
  'interrupted',
]);

export function isTerminated(status: ExchangeStatus): boolean {
  return TERMINATED_STATUSES.has(status);
}

/** The line on a callback waiting behind an open question's answer. Its card
 *  dims, and nothing else in the turn is drawn until the answer. */
export const HELD_CALLBACK_NOTE = 'Not read yet. The agent reads it after you answer below.';

/** The line on an agent-sent message a coding agent holds behind an open
 *  question or permission card (ADR 0256). */
export const HELD_MESSAGE_NOTE = 'Not delivered yet. The coding agent gets it after you answer below.';

/** Map status to a UI label, a CSS class, and for a turn that ended badly a
 *  tooltip saying what happened. A one-word badge cannot say it alone. */
export function statusLabel(
  status: ExchangeStatus,
  hasSteps: boolean,
): { label: string; className: string; tooltip?: string } {
  switch (status) {
    case 'queued':
      return { label: 'Queued', className: 'queued' };
    case 'held':
      return { label: 'Not read yet', className: 'queued' };
    case 'pending':
    case 'streaming':
      return hasSteps
        ? { label: 'Working', className: 'working' }
        : { label: 'Requesting', className: 'working' };
    case 'coding-agent-working':
      return { label: 'Working', className: 'working' };
    case 'awaiting-answer':
      return { label: 'Needs your answer', className: 'awaiting' };
    case 'done':
      return { label: 'Done', className: 'done' };
    case 'interrupted':
      return {
        label: 'Done',
        className: 'done',
        tooltip: 'A newer turn started before this reply finished. The work carried on below.',
      };
    case 'canceled':
      return { label: 'Canceled', className: 'canceled', tooltip: 'You stopped this reply.' };
    case 'error':
      return { label: 'Error', className: 'error', tooltip: 'This reply failed. The note below says why.' };
    case 'aborted':
      return { label: 'Aborted', className: 'aborted', tooltip: 'Lucidos stopped this reply. The next card says why.' };
  }
}
