import { useDelayedFlag } from './useDelayedLoading';

/** What the picked choice draws in its mark slot: its radio dot or check. */
export type PickMark = 'none' | 'spinner' | 'mark';

/** The picked choice's mark only moves forward: nothing while the pick is
 *  unconfirmed, a spinner once that wait passes the delay gate, then the mark.
 *  Never pass back over a mark the reader has seen, or a confirm landing just
 *  past the gate reads as a blink. */
export function usePickMark(confirmed: boolean, pending: boolean): PickMark {
  const sending = useDelayedFlag(!confirmed && pending);
  return confirmed ? 'mark' : sending ? 'spinner' : 'none';
}
