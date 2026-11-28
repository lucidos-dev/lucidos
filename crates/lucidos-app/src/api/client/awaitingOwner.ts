
export const AWAITING_OWNER = 'awaiting-owner' as const;
export type AwaitingOwner = typeof AWAITING_OWNER;

const AWAITING_OWNER_STATUS = 202;

export const AWAITING_OWNER_RETRY_MS = 3000;

export function isAwaitingOwner(res: Response): boolean {
  return res.status === AWAITING_OWNER_STATUS;
}

export async function untilOwnerAllows<T>(
  attempt: () => Promise<T | AwaitingOwner>,
  onWaiting: () => void,
  signal?: AbortSignal,
  pause: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): Promise<T> {
  for (;;) {
    const answer = await attempt();
    if (answer !== AWAITING_OWNER) return answer;
    onWaiting();
    await pause(AWAITING_OWNER_RETRY_MS);
    if (signal?.aborted) throw new Error('stopped waiting for the owner');
  }
}
