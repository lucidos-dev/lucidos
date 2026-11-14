/**
 * A pairing that waits for a paired device's Allow.
 *
 * A local process can mint a code or a gateway secret. Once any device is
 * paired, that pairing redeems only after a paired device taps Allow under
 * Settings → Access. Until then the gateway answers the redeem with a 202, and
 * the redeemer presents the same secret again. Don't allow turns the next
 * answer into a refusal, which the caller reports like any other.
 */

/** What a redeem answers while nobody has tapped Allow yet. */
export const AWAITING_OWNER = 'awaiting-owner' as const;
export type AwaitingOwner = typeof AWAITING_OWNER;

/** The status a waiting redeem answers with. */
const AWAITING_OWNER_STATUS = 202;

/** How long to wait before presenting the secret again. */
export const AWAITING_OWNER_RETRY_MS = 3000;

/** Did the gateway answer "waiting for the owner"? */
export function isAwaitingOwner(res: Response): boolean {
  return res.status === AWAITING_OWNER_STATUS;
}

/**
 * Run `attempt` until it stops answering [`AWAITING_OWNER`].
 *
 * A retry, not a subscription: the redeemer is unpaired, or is another
 * gateway, so no event stream reaches it. The gateway ends the wait itself.
 * The pairing expires five minutes after it started waiting, and the next
 * attempt is then refused. `onWaiting` runs before each pause, so the screen
 * can say what it waits for. An aborted `signal` rejects at the next pause.
 */
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
