// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { LONG_PRESS_DELAY_MS, makeLongPressHandlers } from '../../../hooks/useLongPress';
import { touchActivated } from '../../../utils/tapGesture';

/** The composer's Send, wired as `PromptInput` wires it: a hold that turns on
 *  side-question mode and marks itself, and a touch-path send that the mark
 *  stands down (`releaseEndsHold`).
 *
 *  The iOS PWA report this pins: a tap on Send or Submit with the keyboard up
 *  delivered a `touchstart` and then no lift at all. The hold's timer read the
 *  silence as a held finger and opened a side question. */
function composerSend() {
  const button = document.createElement('button');
  document.body.append(button);
  const state = { sideQuestionMode: false, sent: 0 };
  let heldSend = false;
  const hold = makeLongPressHandlers(() => {
    heldSend = true;
    state.sideQuestionMode = true;
  }, () => {});
  const activate = touchActivated(() => {
    if (heldSend) { heldSend = false; return; }
    state.sent += 1;
  });
  button.addEventListener('pointerdown', (e) => {
    heldSend = false;
    hold.onPointerDown(e as PointerEvent);
  });
  button.addEventListener('pointerup', (e) => hold.onPointerUp(e as PointerEvent));
  button.addEventListener('touchstart', (e) => activate.onTouchStart(e as unknown as TouchEvent));
  button.addEventListener('touchend', (e) => activate.onTouchEnd(e as unknown as TouchEvent));
  return { button, state };
}

function at<T extends Event>(e: T, timeStamp: number, fields: object = {}): T {
  Object.assign(e, fields);
  return Object.defineProperty(e, 'timeStamp', { value: timeStamp });
}
const contact = { identifier: 1, screenX: 10, screenY: 10, clientX: 10, clientY: 10 };
function press(button: HTMLElement) {
  button.dispatchEvent(at(new Event('pointerdown', { bubbles: true }), 0, { button: 0, pointerId: 1, clientX: 10, clientY: 10 }));
  button.dispatchEvent(at(new Event('touchstart', { bubbles: true }), 0, { changedTouches: [contact], touches: [contact] }));
}
function lift(button: HTMLElement, timeStamp: number) {
  button.dispatchEvent(at(new Event('touchend', { bubbles: true, cancelable: true }), timeStamp, { changedTouches: [contact], touches: [] }));
  button.dispatchEvent(at(new Event('pointerup', { bubbles: true }), timeStamp, { pointerId: 1 }));
}

describe('a hold on Send needs its lift', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => {
    vi.useRealTimers();
    document.body.replaceChildren();
  });

  it('a press whose lift never arrives leaves side-question mode off', () => {
    const { button, state } = composerSend();
    press(button);
    vi.advanceTimersByTime(10 * LONG_PRESS_DELAY_MS);
    expect(state.sideQuestionMode).toBe(false);
  });

  it('a tap sends, and opens no side question', () => {
    const { button, state } = composerSend();
    press(button);
    lift(button, 80);
    vi.advanceTimersByTime(10 * LONG_PRESS_DELAY_MS);
    expect(state).toEqual({ sideQuestionMode: false, sent: 1 });
  });

  it('a held press opens the side question at its lift, and the release sends nothing', () => {
    const { button, state } = composerSend();
    press(button);
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(state.sideQuestionMode).toBe(false);
    lift(button, LONG_PRESS_DELAY_MS);
    expect(state).toEqual({ sideQuestionMode: true, sent: 0 });
  });
});
