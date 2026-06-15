/**
 * Exchange status state machine.
 *
 * Replaces the 4 boolean flags (isProcessing, isPending, wasInterrupted, canceled)
 * with a single enum.
 */

export type ExchangeStatus =
  | 'pending'          // Created, waiting for first SSE event
  | 'queued'           // Waiting for a prior active exchange to finish
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

/** Map status to a UI label and CSS class. */
export function statusLabel(
  status: ExchangeStatus,
  hasSteps: boolean,
): { label: string; className: string } {
  switch (status) {
    case 'queued':
      return { label: 'Queued', className: 'queued' };
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
      return { label: 'Done', className: 'done' };
    case 'canceled':
      return { label: 'Canceled', className: 'canceled' };
    case 'error':
      return { label: 'Error', className: 'error' };
    case 'aborted':
      return { label: 'Aborted', className: 'aborted' };
  }
}
