import { createTapGate, notePressOutcome } from './tapGesture';

/** A menu scroll must never activate a row, even at a scroll limit. Capture
 * before the rows' keyboard-friendly touchend and touchcancel recovery paths.
 * Keep one instance for the panel's lifetime; movement never prevents scrolling.
 */
export function createMenuTapGuard() {
  const gate = createTapGate();
  let identifier: number | null = null;
  let blocked = false;

  function move(e: TouchEvent): void {
    for (const touch of Array.from(e.changedTouches)) {
      if (touch.identifier === identifier) gate.move(touch);
    }
  }

  function refuse(e: Event): void {
    e.preventDefault();
    e.stopPropagation();
    notePressOutcome('refused');
  }

  return {
    onPointerDownCapture(e: PointerEvent): void {
      if (e.pointerType !== 'touch') blocked = false;
    },
    onTouchStartCapture(e: TouchEvent): void {
      // A new press resets a missed lift, for example a row removed by SSE.
      const touch = e.changedTouches[0];
      identifier = touch?.identifier ?? null;
      blocked = false;
      if (touch) gate.down(touch);
    },
    onTouchMoveCapture: move,
    onTouchEndCapture(e: TouchEvent): void {
      move(e);
      blocked = blocked || !gate.isTap();
      identifier = null;
      if (blocked) refuse(e);
    },
    onTouchCancelCapture(e: TouchEvent): void {
      move(e);
      blocked = blocked || !gate.isTap();
      identifier = null;
      if (blocked) refuse(e);
    },
    onClickCapture(e: MouseEvent): void {
      // Keyboard and assistive activation have no physical click count.
      if (blocked && e.detail !== 0) refuse(e);
    },
  };
}
