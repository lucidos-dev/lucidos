// @vitest-environment jsdom
import { h, render } from 'preact';
import { describe, expect, it, vi } from 'vitest';
import { createMenuTapGuard } from './menuTapGuard';
import { touchActivated } from './tapGesture';

function harness() {
  const guard = createMenuTapGuard();
  const action = vi.fn();
  const row = touchActivated(action);
  function touch(type: 'start' | 'move' | 'end' | 'cancel', y: number) {
    const e = {
      touches: type === 'end' || type === 'cancel' ? [] : [{ identifier: 1, screenX: 100, screenY: y }],
      changedTouches: [{ identifier: 1, screenX: 100, screenY: y }],
      preventDefault: vi.fn(), stopPropagation: vi.fn(),
    };
    const capture = { start: guard.onTouchStartCapture, move: guard.onTouchMoveCapture,
      end: guard.onTouchEndCapture, cancel: guard.onTouchCancelCapture }[type];
    capture(e as unknown as TouchEvent);
    if (!e.stopPropagation.mock.calls.length) {
      const handler = { start: row.onTouchStart, move: row.onTouchMove,
        end: row.onTouchEnd, cancel: row.onTouchCancel }[type];
      handler(e);
    }
    return e;
  }
  function click(detail = 1) {
    const e = { detail, preventDefault: vi.fn(), stopPropagation: vi.fn() };
    guard.onClickCapture(e as unknown as MouseEvent);
    if (!e.stopPropagation.mock.calls.length) row.onClick();
  }
  return { guard, action, touch, click };
}

describe('menu tap guard', () => {
  it.each([false, true])('blocks swipes without relying on scroll offsets (returns to origin: %s)', back => {
    const h = harness();
    h.touch('start', 200);
    expect(h.touch('move', 100).preventDefault).not.toHaveBeenCalled();
    h.touch('end', back ? 200 : 100);
    h.click();
    expect(h.action).not.toHaveBeenCalled();
  });

  it('measures the lift even without a touchmove', () => {
    const h = harness();
    h.touch('start', 200);
    h.touch('end', 100);
    h.click();
    expect(h.action).not.toHaveBeenCalled();
  });

  it('does not let cancelled swipes reach the row recovery path', () => {
    const h = harness();
    h.touch('start', 200);
    h.touch('cancel', 100);
    h.click();
    expect(h.action).not.toHaveBeenCalled();
  });

  it('serves a tap once, including the next tap after a swipe', () => {
    const h = harness();
    h.touch('start', 200);
    h.touch('end', 100);
    h.touch('start', 200);
    h.touch('end', 200);
    h.click();
    expect(h.action).toHaveBeenCalledTimes(1);
  });

  it.each(['mouse', 'pen'])('preserves keyboard activation and a fresh %s press after scrolling', pointerType => {
    const h = harness();
    h.touch('start', 200);
    h.touch('end', 100);
    h.click(0);
    expect(h.action).toHaveBeenCalledTimes(1);
    h.guard.onPointerDownCapture({ pointerType } as PointerEvent);
    h.click();
    expect(h.action).toHaveBeenCalledTimes(2);
  });
});

it('captures a swipe before a rendered menu row can activate', () => {
  const host = document.createElement('div');
  const action = vi.fn();
  render(h('div', { ...createMenuTapGuard(), role: 'menu' },
    h('button', { ...touchActivated(action), role: 'menuitem' }, 'Setup guide')), host);
  const button = host.querySelector('button')!;
  function dispatch(type: string, y: number) {
    const event = new Event(type, { bubbles: true, cancelable: true });
    const contact = { identifier: 1, screenX: 100, screenY: y };
    Object.defineProperties(event, {
      touches: { value: type === 'touchend' ? [] : [contact] },
      changedTouches: { value: [contact] },
    });
    button.dispatchEvent(event);
  }
  dispatch('touchstart', 200);
  dispatch('touchmove', 100);
  dispatch('touchend', 200);
  button.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
  expect(action).not.toHaveBeenCalled();
  dispatch('touchstart', 200);
  dispatch('touchend', 200);
  expect(action).toHaveBeenCalledTimes(1);
  render(null, host);
});

it('preserves recovery for a stationary cancelled tap', () => {
  const h = harness();
  h.touch('start', 200);
  h.touch('cancel', 200);
  expect(h.action).toHaveBeenCalledTimes(1);
});

it('a new press recovers even when the preceding row lost its lift', () => {
  const h = harness();
  h.touch('start', 200);
  h.touch('move', 100);
  h.touch('start', 200);
  h.touch('end', 200);
  expect(h.action).toHaveBeenCalledTimes(1);
});

it('tracks the row contact while another finger rests elsewhere', () => {
  const guard = createMenuTapGuard();
  const other = { identifier: 99, screenX: 0, screenY: 0 };
  const row = { identifier: 1, screenX: 100, screenY: 200 };
  guard.onTouchStartCapture({ touches: [other, row], changedTouches: [row] } as unknown as TouchEvent);
  const stopPropagation = vi.fn();
  guard.onTouchEndCapture({ touches: [other], changedTouches: [row],
    preventDefault: vi.fn(), stopPropagation } as unknown as TouchEvent);
  expect(stopPropagation).not.toHaveBeenCalled();
});
