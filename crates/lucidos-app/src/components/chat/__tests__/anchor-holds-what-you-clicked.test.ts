import { describe, it, expect, beforeEach, vi } from 'vitest';

if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}
if (typeof globalThis.document !== 'undefined' && !('activeElement' in globalThis.document)) {
  (globalThis.document as any).activeElement = null;
}
if (typeof (globalThis as any).requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}
if (typeof (globalThis as any).cancelAnimationFrame === 'undefined') {
  (globalThis as any).cancelAnimationFrame = () => {};
}

import { mockContainer, mockDynamicAnchor, useMockMO } from './scroll-test-helpers';
import { withScrollAnchor } from '../CreateThreadView';
import { resumeFollowingBottom, setActiveScrollElement, stopFollowingBottom } from '../scrollState';
import { DISCLOSURE_MAX_MS } from '../../../utils/disclosureMotion';


function viewportTop(el: { getBoundingClientRect: () => { top: number } }): number {
  return el.getBoundingClientRect().top;
}

describe('a turn control holds the element the reader clicked', () => {
  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  it('holds it when the reveal grows the turns ABOVE it', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 1000, scrollHeight: 6000 });
    const control = mockDynamicAnchor(container, 1300);
    const before = viewportTop(control);

    withScrollAnchor(control as any, () => {
      control._setOffset(1800);
      container.scrollHeight = 9000;
    });

    expect(viewportTop(control)).toBe(before);
    expect(container.scrollTop).toBe(1500);
    restoreMO();
  });

  it('holds it when the reveal SHRINKS the turns above it', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 4000, scrollHeight: 9000 });
    const control = mockDynamicAnchor(container, 4300);
    const before = viewportTop(control);

    withScrollAnchor(control as any, () => {
      control._setOffset(1800);
      container.scrollHeight = 6000;
    });

    expect(viewportTop(control)).toBe(before);
    expect(container.scrollTop).toBe(1500);
    restoreMO();
  });

  it('keeps holding it while the rows above are still rolling', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 1000, scrollHeight: 9000 });
    const control = mockDynamicAnchor(container, 1300);
    const before = viewportTop(control);
    const frames: FrameRequestCallback[] = [];
    const syncFrame = globalThis.requestAnimationFrame;
    (globalThis as any).requestAnimationFrame = (cb: FrameRequestCallback) => { frames.push(cb); return 0; };
    try {
      withScrollAnchor(control as any, () => { control._setOffset(1310); });
      const start = performance.now();
      for (let frame = 1; frame <= 48 && frames.length > 0; frame++) {
        control._setOffset(1310 + (frame / 48) * 490);
        frames.shift()!(start + frame * 8.33);
      }
      expect(viewportTop(control)).toBe(before);
    } finally {
      (globalThis as any).requestAnimationFrame = syncFrame;
      restoreMO();
    }
  });

  it('holds it until a roll that started after a slow render has landed', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 1000, scrollHeight: 9000 });
    const control = mockDynamicAnchor(container, 1300);
    const before = viewportTop(control);
    const frames: FrameRequestCallback[] = [];
    const syncFrame = globalThis.requestAnimationFrame;
    (globalThis as any).requestAnimationFrame = (cb: FrameRequestCallback) => { frames.push(cb); return 0; };
    const pressedAt = performance.now();
    const renderMs = 300;
    const clock = vi.spyOn(performance, 'now').mockReturnValue(pressedAt);
    try {
      withScrollAnchor(control as any, () => {
        clock.mockReturnValue(pressedAt + renderMs);
        control._setOffset(1310);
      });
      const rollStart = pressedAt + renderMs;
      const rollFrames = Math.ceil(DISCLOSURE_MAX_MS / 8.33);
      for (let frame = 1; frame <= rollFrames && frames.length > 0; frame++) {
        control._setOffset(1310 + (frame / rollFrames) * 490);
        frames.shift()!(rollStart + frame * 8.33);
      }
      expect(viewportTop(control)).toBe(before);
    } finally {
      clock.mockRestore();
      (globalThis as any).requestAnimationFrame = syncFrame;
      restoreMO();
    }
  });

  it('writes nothing when only the content BELOW it changed', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 800, scrollHeight: 4000 });
    const control = mockDynamicAnchor(container, 1100);
    const before = viewportTop(control);

    withScrollAnchor(control as any, () => { container.scrollHeight = 7000; });

    expect(container.scrollTop).toBe(800);
    expect(viewportTop(control)).toBe(before);
    restoreMO();
  });

  it('leaves the reader alone when the mutation takes the control away', () => {
    const restoreMO = useMockMO();
    const container = mockContainer({ scrollTop: 900, scrollHeight: 4000 });
    const stub = mockDynamicAnchor(container, 1200);

    withScrollAnchor(stub as any, () => {
      (stub as any).isConnected = false;
      (stub as any).getBoundingClientRect = () => ({ top: 0, bottom: 0, height: 0, left: 0, right: 0, width: 0 });
      container.scrollHeight = 6000;
    });

    expect(container.scrollTop).toBe(900);
    restoreMO();
  });

  describe('with the reader on the end of a quiet thread', () => {
    function armedAtTheEnd(container: ReturnType<typeof mockContainer>) {
      setActiveScrollElement(container as any);
      resumeFollowingBottom(container as any);
    }

    it('holds the control the reader pressed', () => {
      const restoreMO = useMockMO();
      const container = mockContainer({ scrollTop: 3500, scrollHeight: 4000 });
      armedAtTheEnd(container);
      const control = mockDynamicAnchor(container, 3600);
      const before = viewportTop(control);

      withScrollAnchor(control as any, () => {
        control._setOffset(4100);
        container.scrollHeight = 9000;
      });

      expect(viewportTop(control)).toBe(before);
      expect(container.scrollTop).toBe(4000);
      restoreMO();
    });

    it('moves nobody when the pressed control leaves with the mutation', () => {
      const restoreMO = useMockMO();
      const container = mockContainer({ scrollTop: 3500, scrollHeight: 4000 });
      armedAtTheEnd(container);
      const stub = mockDynamicAnchor(container, 3600);

      withScrollAnchor(stub as any, () => {
        (stub as any).isConnected = false;
        (stub as any).getBoundingClientRect = () => ({ top: 0, bottom: 0, height: 0, left: 0, right: 0, width: 0 });
        container.scrollHeight = 9000;
      });

      expect(container.scrollTop).toBe(3500);
      restoreMO();
    });
  });
});
