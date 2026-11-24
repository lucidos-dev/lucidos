export type FrameState = 'unknown' | 'off-screen' | 'loading' | 'revealed';

export function widgetFrameState(inView: boolean | null, showing: boolean, revealed: boolean): FrameState {
  if (inView === null) return 'unknown';
  if (revealed) return 'revealed';
  return showing ? 'loading' : 'off-screen';
}
