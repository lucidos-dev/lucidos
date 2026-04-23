/**
 * Exchange status state machine.
 *
 * Replaces the 4 boolean flags (isProcessing, isPending, wasInterrupted, canceled)
 * with a single enum.
 */

export type ExchangeStatus =
  | 'pending'      // Created, waiting for first SSE event
  | 'queued'       // Waiting for a prior active exchange to finish
  | 'streaming'    // SSE events flowing (text/tools)
  | 'cc-working'   // Claude Code actively working
  | 'done'         // Response complete
  | 'interrupted'  // User sent follow-up while streaming
  | 'canceled'     // User explicitly canceled
  | 'error'        // Request failed
  | 'aborted';     // System interrupted (engine restart/crash mid-response)

export const ACTIVE_STATUSES: Set<ExchangeStatus> = new Set(['pending', 'streaming', 'cc-working']);

export function isActive(status: ExchangeStatus): boolean {
  return ACTIVE_STATUSES.has(status);
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
    case 'cc-working':
      return { label: 'Working', className: 'working' };
    case 'done':
      return { label: 'Done', className: 'done' };
    case 'interrupted':
      return { label: 'Continued below', className: 'done' };
    case 'canceled':
      return { label: 'Canceled', className: 'canceled' };
    case 'error':
      return { label: 'Error', className: 'error' };
    case 'aborted':
      return { label: 'Aborted', className: 'aborted' };
  }
}
