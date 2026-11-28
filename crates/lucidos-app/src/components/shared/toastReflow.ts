export interface ToastTop {
  id: number;
  top: number;
}

export interface ToastShift {
  id: number;
  delta: number;
}

const MOVEMENT_THRESHOLD_PX = 1;

export function computeToastShifts(
  prevIds: ReadonlySet<number>,
  oldTops: ReadonlyMap<number, number>,
  current: readonly ToastTop[],
  threshold = MOVEMENT_THRESHOLD_PX,
): ToastShift[] {
  const shifts: ToastShift[] = [];
  for (const { id, top } of current) {
    if (!prevIds.has(id)) continue;
    const oldTop = oldTops.get(id);
    if (oldTop === undefined) continue;
    const delta = oldTop - top;
    if (Math.abs(delta) < threshold) continue;
    shifts.push({ id, delta });
  }
  return shifts;
}
