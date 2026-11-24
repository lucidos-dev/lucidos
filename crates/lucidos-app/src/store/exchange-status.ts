
export type ExchangeStatus =
  | 'pending'
  | 'queued'
  | 'held'
  | 'streaming'
  | 'coding-agent-working'
  | 'awaiting-answer'
  | 'done'
  | 'interrupted'
  | 'canceled'
  | 'error'
  | 'aborted';

export const ACTIVE_STATUSES: Set<ExchangeStatus> = new Set(['pending', 'streaming', 'coding-agent-working']);

export function isActive(status: ExchangeStatus): boolean {
  return ACTIVE_STATUSES.has(status);
}

export const TERMINATED_STATUSES: Set<ExchangeStatus> = new Set([
  'canceled',
  'aborted',
  'error',
  'interrupted',
]);

export function isTerminated(status: ExchangeStatus): boolean {
  return TERMINATED_STATUSES.has(status);
}

export const HELD_CALLBACK_NOTE = 'Not read yet. The agent reads it after you answer below.';

export const QUEUED_BEHIND_CARD_LABEL = 'Queued until you answer the open card';

export const HELD_MESSAGE_NOTE = 'Not delivered yet. The coding agent gets it after you answer below.';

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
