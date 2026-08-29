import { describe, it, expect, beforeEach } from 'vitest';

// Stub HTMLElement before importing the modules that reference it, exactly as
// the sibling scroll suites do.
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
import { resumeFollowingBottom, setActiveScrollElement, setThreadLive, stopFollowingBottom } from '../scrollState';

/**
 * **The anchor is the element the reader clicked.**
 *
 * A turn control changes heights and nothing else, and the reader asked for
 * that by pressing one named thing. So that thing is what must not move.
 * `ChatExchange`'s `heldOnThePress` hands the control over as the click's own
 * `currentTarget`, and this suite is the arithmetic that keeps it still.
 *
 * The assertion is always the control's own VIEWPORT position, never a
 * `scrollTop` number. That is the contract as the reader experiences it, and it
 * fails loudly whichever of the two readings drifts.
 *
 * The rule this replaced ranked candidates, preferring the reader's topmost
 * visible line. It left a reader at the top of the thread for pressing the
 * second turn's control. See
 * docs/plans/2026-08-28-a-turn-control-holds-what-you-pressed.md
 */

/** Where the control sits on screen, which is the whole of what must not move. */
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
    // The reader is 1000px down a long transcript, with the control they are
    // about to press 300px below their edge.
    const container = mockContainer({ scrollTop: 1000, scrollHeight: 6000 });
    const control = mockDynamicAnchor(container, 1300);
    const before = viewportTop(control);

    // Showing the steps puts 500px of rows above the control. Every earlier turn
    // grew too, which is exactly what the old rule turned into a jump.
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

  it('writes nothing when only the content BELOW it changed', () => {
    const restoreMO = useMockMO();
    // Pressing the LAST turn's control grows only what is under it. The freeze
    // has already left the reader right, so a write here would be motion for
    // nothing.
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
    // The `⋯` stub is replaced by the body it reveals, so the anchor is gone by
    // the time the correction runs. A detached node does not say so by measuring
    // nothing: it answers an all-zero rect, which reads as content at the very
    // top of the thread. Correcting against that would send the reader there.
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

  /** **A reader parked on the end of a quiet thread.**
   *
   *  The one park that used to be exempt. An armed reader on the live edge was
   *  carried to the new edge rather than held, which moved the icon they had
   *  just pressed. Nothing is arriving on a quiet thread, so the ride has
   *  nothing to carry them toward and the press wins.
   *
   *  Armed through `resumeFollowingBottom` rather than the toggle: it arms and
   *  writes the edge with no tween, and this file runs `requestAnimationFrame`
   *  synchronously. */
  describe('with the reader on the end of a quiet thread', () => {
    function armedAtTheEnd(container: ReturnType<typeof mockContainer>) {
      setActiveScrollElement(container as any);
      setThreadLive(false);
      resumeFollowingBottom(container as any);
    }

    it('holds the control the reader pressed', () => {
      const restoreMO = useMockMO();
      const container = mockContainer({ scrollTop: 3500, scrollHeight: 4000 });
      armedAtTheEnd(container);
      // The last turn is short, so its header IS on screen at the bottom. That
      // is the only way a finger reaches a control from here.
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

    it('moves nobody when the `⋯` stub unfolds under them', () => {
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
