import { useDelayedFlag } from './useDelayedLoading';

export type PickMark = 'none' | 'spinner' | 'mark';

export function usePickMark(confirmed: boolean, pending: boolean): PickMark {
  const sending = useDelayedFlag(!confirmed && pending);
  return confirmed ? 'mark' : sending ? 'spinner' : 'none';
}
