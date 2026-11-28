
const PX_PER_NOTCH = 100;

const LINES_PER_NOTCH = 3;

const NOTCHES_PER_PAGE = 8;

export const WHEEL_GESTURE_GAP_MS = 150;

export interface WheelBank {
  distance: number;
  lastEventAt: number;
}

export function emptyWheelBank(): WheelBank {
  return { distance: 0, lastEventAt: 0 };
}

export function wheelZoomDistance(deltaY: number, deltaMode: number): number {
  if (!Number.isFinite(deltaY)) return 0;
  switch (deltaMode) {
    case 1: return -deltaY * (PX_PER_NOTCH / LINES_PER_NOTCH);
    case 2: return -deltaY * PX_PER_NOTCH * NOTCHES_PER_PAGE;
    default: return -deltaY;
  }
}

export function foldWheelEvent(
  bank: WheelBank, distance: number, nowMs: number, modifierKeyHeld: boolean,
): WheelBank {
  if (distance === 0) return bank;
  const stale = nowMs - bank.lastEventAt > WHEEL_GESTURE_GAP_MS;
  const reversed = bank.distance !== 0
    && Math.sign(distance) !== Math.sign(bank.distance);
  if (!(stale || reversed)) {
    return { distance: bank.distance + distance, lastEventAt: nowMs };
  }
  const floor = modifierKeyHeld ? PX_PER_NOTCH : 0;
  const opening = Math.sign(distance) * Math.max(Math.abs(distance), floor);
  return { distance: opening, lastEventAt: nowMs };
}

export function takeWheelStep(bank: WheelBank): { bank: WheelBank; step: number } {
  if (Math.abs(bank.distance) < PX_PER_NOTCH) return { bank, step: 0 };
  const step = Math.sign(bank.distance);
  return {
    bank: { ...bank, distance: bank.distance - step * PX_PER_NOTCH },
    step,
  };
}

export function createWheelZoom(step: (direction: number) => void) {
  let bank = emptyWheelBank();
  let frame: number | null = null;

  function drain(): void {
    frame = null;
    const taken = takeWheelStep(bank);
    bank = taken.bank;
    if (taken.step === 0) return;
    step(taken.step);
    schedule();
  }

  function schedule(): void {
    if (frame !== null) return;
    frame = requestAnimationFrame(drain);
  }

  return {
    push(deltaY: number, deltaMode: number, at: number, modifierKeyHeld: boolean): void {
      bank = foldWheelEvent(bank, wheelZoomDistance(deltaY, deltaMode), at, modifierKeyHeld);
      schedule();
    },
    stop(): void {
      bank = emptyWheelBank();
      if (frame === null) return;
      cancelAnimationFrame(frame);
      frame = null;
    },
  };
}
