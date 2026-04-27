import { describe, it, expect } from 'vitest';
import { computeAnchorPosition, isOutsidePointerTarget } from './useAnchoredPopover';

function fakeAnchor(rect: { top: number; bottom: number; left: number; right: number }): HTMLElement {
  return { getBoundingClientRect: () => rect } as unknown as HTMLElement;
}

function setViewport(width: number, height: number) {
  Object.defineProperty(window, 'innerWidth', { value: width, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: height, configurable: true });
}

/** Build a fake element whose `contains` answers true for itself and a fixed
 *  list of descendant nodes — enough to drive isOutsidePointerTarget without jsdom. */
function elWith(descendants: Node[] = []): HTMLElement {
  const self: any = {};
  self.contains = (n: Node) => n === self || descendants.includes(n);
  return self as HTMLElement;
}

describe('computeAnchorPosition', () => {
  it('places below when there is room', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 50, right: 100 }), 200, 300);
    expect(pos.placement).toBe('bottom-start');
    expect(pos.top).toBe(124);
    expect(pos.left).toBe(50);
  });

  it('flips to top when below would overflow viewport', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(fakeAnchor({ top: 700, bottom: 720, left: 50, right: 100 }), 200, 300);
    expect(pos.placement).toBe('top-start');
    expect(pos.top).toBe(496);
  });

  it('keeps left aligned with anchor when there is horizontal room', () => {
    setViewport(1280, 800);
    const below = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 30, right: 80 }), 200, 300);
    const above = computeAnchorPosition(fakeAnchor({ top: 700, bottom: 720, left: 30, right: 80 }), 200, 300);
    expect(below.left).toBe(30);
    expect(above.left).toBe(30);
  });

  it('shifts left so the panel fits when anchor is near the right edge', () => {
    setViewport(393, 852); // iPhone 14 Pro
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 350, right: 380 }), 200, 320);
    // Without clamping, left would be 350 and the 320px panel would extend to 670 — far off-screen.
    // Clamped: 393 - 320 - 8 = 65
    expect(pos.left).toBe(65);
  });

  it('clamps to the left margin when the panel is wider than the viewport allows', () => {
    setViewport(360, 800);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 300, right: 340 }), 200, 380);
    // Panel wider than fits — pin to the left margin instead of overflowing.
    expect(pos.left).toBe(8);
  });

  it('clamps left to the container bounds when a container is given', () => {
    setViewport(1280, 800);
    // Chat pane occupies the left 500px of a 1280px viewport. Anchor near the
    // pane's right edge would otherwise push the 320px panel into the content
    // pane on the right; the container clamp keeps it inside the chat pane.
    const container = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 500 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 460, right: 490 }),
      200,
      320,
      container,
    );
    // Clamped: 500 - 320 - 8 = 172
    expect(pos.left).toBe(172);
  });

  it('keeps left aligned with anchor when the container has room', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 500 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 80, right: 130 }),
      200,
      300,
      container,
    );
    expect(pos.left).toBe(80);
  });
});

describe('isOutsidePointerTarget', () => {
  it('returns false when the click is inside the panel', () => {
    const panel = elWith();
    const anchor = elWith();
    expect(isOutsidePointerTarget(panel, panel, anchor)).toBe(false);
  });

  it('returns false when the click is inside the anchor (toggle case)', () => {
    const panel = elWith();
    const anchor = elWith();
    expect(isOutsidePointerTarget(anchor, panel, anchor)).toBe(false);
  });

  it('returns true when the click is on a sibling element', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    expect(isOutsidePointerTarget(elsewhere, panel, anchor)).toBe(true);
  });

  it('still treats descendants of the panel as inside', () => {
    const child = elWith();
    const panel = elWith([child]);
    const anchor = elWith();
    expect(isOutsidePointerTarget(child, panel, anchor)).toBe(false);
  });

  it('still treats descendants of the anchor as inside', () => {
    const child = elWith();
    const panel = elWith();
    const anchor = elWith([child]);
    expect(isOutsidePointerTarget(child, panel, anchor)).toBe(false);
  });

  it('treats null panel as fully outside', () => {
    const anchor = elWith();
    const elsewhere = elWith();
    expect(isOutsidePointerTarget(elsewhere, null, anchor)).toBe(true);
  });
});
