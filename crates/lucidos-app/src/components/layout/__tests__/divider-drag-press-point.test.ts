// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { startDividerDrag } from '../dividerDrag';

function pointer(type: string, clientX: number, clientY: number): Event {
  return Object.assign(new Event(type), { clientX, clientY, pointerId: 1 });
}

function press(handle: HTMLElement, clientX: number, clientY: number): PointerEvent {
  const e = pointer('pointerdown', clientX, clientY);
  Object.defineProperty(e, 'currentTarget', { value: handle });
  return e as unknown as PointerEvent;
}

function makeHandle(): HTMLElement {
  const handle = document.createElement('div');
  handle.setPointerCapture = () => {};
  document.body.append(handle);
  return handle;
}

describe('a divider drag', () => {
  let handle: HTMLElement | undefined;

  afterEach(() => {
    handle?.remove();
    handle = undefined;
  });

  it('does not resize on a move that has not left the press point', () => {
    handle = makeHandle();
    const onMove = vi.fn();
    const end = startDividerDrag(press(handle, 100, 100), onMove, () => {});
    handle.dispatchEvent(pointer('pointermove', 100, 100));
    expect(onMove).not.toHaveBeenCalled();
    handle.dispatchEvent(pointer('pointermove', 120, 100));
    expect(onMove).toHaveBeenCalledTimes(1);
    end();
  });
});
