import { afterEach, describe, it, expect, vi } from 'vitest';
import {
  anchoredPanelStyle, computeAnchorPosition, isOutsidePointerTarget, makeDismissHandlers,
  installPairedSwallow, naturalPanelHeight, pointAnchor,
} from './useAnchoredPopover';
import { notePressOutcome, takePressOutcome } from '../utils/tapGesture';

function fakeAnchor(rect: { top: number; bottom: number; left: number; right: number }): HTMLElement {
  return { getBoundingClientRect: () => rect } as unknown as HTMLElement;
}

function setViewport(width: number, height: number) {
  Object.defineProperty(window, 'innerWidth', { value: width, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: height, configurable: true });
}

function elWith(descendants: Node[] = []): HTMLElement {
  const self: any = {};
  self.contains = (n: Node) => n === self || descendants.includes(n);
  return self as HTMLElement;
}

function fakeElement(inToast: boolean): HTMLElement {
  return {
    nodeType: 1,
    closest: (sel: string) => (inToast && sel === '.toast-container' ? {} : null),
  } as unknown as HTMLElement;
}

function toastButton(): HTMLElement {
  return fakeElement(true);
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
    setViewport(393, 852);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 350, right: 380 }), 200, 320);
    expect(pos.left).toBe(65);
  });

  it('pulls a wide panel back from a left-of-centre anchor on a phone', () => {
    setViewport(393, 852);
    const clockButton = fakeAnchor({ top: 760, bottom: 796, left: 78, right: 114 });
    const pos = computeAnchorPosition(clockButton, 300, 375);
    expect(pos.left).toBe(10);
    expect(pos.placement).toBe('top-start');
  });

  it('keeps a flipped panel on screen when there is not enough room above', () => {
    setViewport(393, 400);
    const clockButton = fakeAnchor({ top: 300, bottom: 336, left: 78, right: 114 });
    const pos = computeAnchorPosition(clockButton, 324, 320);
    expect(pos.top).toBe(8);
    expect(pos.placement).toBe('top-start');
  });

  describe('with the keyboard up', () => {
    function setVisualViewport(vv: { height: number; offsetTop: number } | undefined) {
      Object.defineProperty(window, 'visualViewport', { value: vv, configurable: true });
    }

    afterEach(() => setVisualViewport(undefined));

    it('flips above rather than opening behind the keyboard', () => {
      setViewport(393, 852);
      setVisualViewport({ height: 486, offsetTop: 0 });
      const pos = computeAnchorPosition(fakeAnchor({ top: 430, bottom: 466, left: 20, right: 56 }), 342, 240);
      expect(pos.placement).toBe('top-start');
      expect(pos.top).toBe(430 - 342 - 4);
    });

    it('clamps the head to the visible band when the page is panned', () => {
      setViewport(393, 852);
      setVisualViewport({ height: 400, offsetTop: 100 });
      const pos = computeAnchorPosition(fakeAnchor({ top: 300, bottom: 336, left: 20, right: 56 }), 324, 240);
      expect(pos.top).toBe(108);
    });

    it('measures against the layout viewport when the visual one reports no height', () => {
      setViewport(393, 852);
      setVisualViewport({ height: 0, offsetTop: 0 });
      const pos = computeAnchorPosition(fakeAnchor({ top: 400, bottom: 436, left: 20, right: 56 }), 300, 240);
      expect(pos.placement).toBe('bottom-start');
      expect(pos.top).toBe(436 + 4);
    });
  });

  describe('under the status bar', () => {
    const island = 59;

    it('never pins a panel head inside the top safe area', () => {
      setViewport(393, 400);
      const pos = computeAnchorPosition(fakeAnchor({ top: 300, bottom: 336, left: 20, right: 56 }), 324, 240, null, 'start', island);
      expect(pos.top).toBe(island + 8);
    });

    it('caps the panel to the room above the anchor, so it scrolls instead of covering it', () => {
      setViewport(393, 400);
      const anchor = fakeAnchor({ top: 300, bottom: 336, left: 20, right: 56 });
      const pos = computeAnchorPosition(anchor, 600, 240, null, 'start', island);
      expect(pos.placement).toBe('top-start');
      expect(pos.maxHeight).toBe(300 - 4 - (island + 8));
      const capped = computeAnchorPosition(anchor, pos.maxHeight, 240, null, 'start', island);
      expect(capped.top).toBe(island + 8);
      expect(capped.top + pos.maxHeight).toBe(300 - 4);
    });
  });

  it('opens on the roomier side when the panel fits neither', () => {
    setViewport(393, 852);
    const anchor = fakeAnchor({ top: 200, bottom: 236, left: 20, right: 56 });
    const pos = computeAnchorPosition(anchor, 900, 240);
    expect(pos.placement).toBe('bottom-start');
    expect(pos.maxHeight).toBe(852 - 8 - 240);
    const capped = computeAnchorPosition(anchor, pos.maxHeight, 240);
    expect(capped.placement).toBe('bottom-start');
    expect(capped.top).toBe(240);
  });

  it('keeps a panel capped above on top when the room below is nearly as big', () => {
    setViewport(393, 600);
    const anchor = fakeAnchor({ top: 300, bottom: 336, left: 20, right: 56 });
    const pos = computeAnchorPosition(anchor, 400, 240);
    expect(pos.placement).toBe('top-start');
    const capped = computeAnchorPosition(anchor, 254, 240);
    expect(capped.placement).toBe('top-start');
  });

  it('measures a panel without the room cap it wears', () => {
    const props = new Map<string, string>([['--anchor-room', '252px']]);
    const panel = {
      style: {
        getPropertyValue: (k: string) => props.get(k) ?? '',
        removeProperty: (k: string) => { props.delete(k); return ''; },
        setProperty: (k: string, v: string) => { props.set(k, v); },
      },
      get offsetHeight() { return props.has('--anchor-room') ? 252 : 300; },
    } as unknown as HTMLElement;
    expect(naturalPanelHeight(panel)).toBe(300);
    expect(props.get('--anchor-room')).toBe('252px');
  });

  it('keeps the scroll position of a capped panel it measures', () => {
    const props = new Map<string, string>([['--anchor-room', '252px']]);
    let scrollTop = 48;
    const panel = {
      style: {
        getPropertyValue: (k: string) => props.get(k) ?? '',
        removeProperty: (k: string) => { props.delete(k); scrollTop = 0; return ''; },
        setProperty: (k: string, v: string) => { props.set(k, v); },
      },
      get offsetHeight() { return props.has('--anchor-room') ? 252 : 300; },
      get scrollTop() { return scrollTop; },
      set scrollTop(v: number) { scrollTop = v; },
    } as unknown as HTMLElement;
    naturalPanelHeight(panel);
    expect(panel.scrollTop).toBe(48);
  });

  it('never caps a panel that fits below it', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 50, right: 100 }), 200, 300);
    expect(pos.maxHeight).toBeGreaterThanOrEqual(200);
  });

  it('hands the container cap to the panel style, and hides it until measured', () => {
    expect(anchoredPanelStyle(null)).toEqual({ visibility: 'hidden' });
    expect(anchoredPanelStyle({ top: 84, left: 12, placement: 'top-start', maxWidth: 484, maxHeight: 300 }))
      .toEqual({ top: '84px', left: '12px', maxWidth: '484px' });
  });

  it('clamps to the left margin when the panel is wider than the viewport allows', () => {
    setViewport(360, 800);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 300, right: 340 }), 200, 380);
    expect(pos.left).toBe(8);
  });

  it('clamps left to the container bounds when a container is given', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 500 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 460, right: 490 }),
      200,
      320,
      container,
    );
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

  it('reports the container width, margins deducted, as the panel fit', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 500 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 80, right: 130 }),
      200,
      300,
      container,
    );
    expect(pos.maxWidth).toBe(484);
  });

  it('falls back to the viewport width for the fit when no container is given', () => {
    setViewport(393, 852);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 78, right: 114 }), 200, 300);
    expect(pos.maxWidth).toBe(377);
  });

  it('falls back to the viewport when the container has collapsed to nothing', () => {
    setViewport(1280, 800);
    const collapsed = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 0 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 0, right: 0 }),
      200,
      300,
      collapsed,
    );
    expect(pos.maxWidth).toBe(1264);
  });

  it('pins an over-wide panel where the capped panel will sit', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 200, right: 700 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 240, right: 276 }),
      200,
      900,
      container,
    );
    expect(pos.left).toBe(208);
    expect(pos.maxWidth).toBe(484);
    expect(pos.left + pos.maxWidth).toBe(692);
  });

  it('re-derives top from the panel height, so a reflowed panel clears its anchor', () => {
    setViewport(1280, 900);
    const clockButton = fakeAnchor({ top: 800, bottom: 836, left: 300, right: 336 });
    const container = fakeAnchor({ top: 0, bottom: 900, left: 200, right: 700 }) as HTMLElement;
    const uncapped = computeAnchorPosition(clockButton, 120, 900, container);
    const reflowed = computeAnchorPosition(clockButton, 220, 484, container);
    expect(uncapped.placement).toBe('top-start');
    expect(reflowed.placement).toBe('top-start');
    expect(uncapped.top + 220).toBeGreaterThan(clockButton.getBoundingClientRect().top);
    expect(reflowed.top + 220).toBeLessThanOrEqual(clockButton.getBoundingClientRect().top);
  });

  it("align 'end' aligns the panel's right edge with the anchor's right edge", () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 1010, right: 1040 }),
      200,
      300,
      null,
      'end',
    );
    expect(pos.left).toBe(740);
    expect(pos.left + 300).toBe(1040);
  });

  it("align 'end' stays anchored to the trigger on a narrow mobile viewport (the bug fix)", () => {
    setViewport(393, 852);
    const anchor = fakeAnchor({ top: 100, bottom: 120, left: 350, right: 380 });
    const start = computeAnchorPosition(anchor, 200, 320);
    const end = computeAnchorPosition(anchor, 200, 320, null, 'end');
    expect(start.left).toBe(65);
    expect(end.left).toBe(60);
    expect(end.left + 320).toBe(380);
  });

  it("align 'end' still clamps to the left margin when the panel is wider than the viewport", () => {
    setViewport(360, 800);
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 320, right: 350 }),
      200,
      380,
      null,
      'end',
    );
    expect(pos.left).toBe(8);
  });

  it("align 'container-center' centres the panel in its container, wherever the anchor is", () => {
    setViewport(393, 852);
    const leftTrigger = fakeAnchor({ top: 700, bottom: 720, left: 40, right: 70 });
    const rightTrigger = fakeAnchor({ top: 700, bottom: 720, left: 330, right: 360 });
    expect(computeAnchorPosition(leftTrigger, 300, 361, null, 'container-center').left).toBe(16);
    expect(computeAnchorPosition(rightTrigger, 300, 361, null, 'container-center').left).toBe(16);
  });

  it("align 'container-center' centres on the container, not the viewport", () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 200, right: 800 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 220, right: 250 }),
      200,
      400,
      container,
      'container-center',
    );
    expect(pos.left).toBe(300);
  });

  it("align 'center' centres the panel under its anchor, and still clamps", () => {
    setViewport(393, 852);
    const middle = fakeAnchor({ top: 50, bottom: 80, left: 150, right: 190 });
    expect(computeAnchorPosition(middle, 100, 200, null, 'center').left).toBe(70);
    const nearEdge = fakeAnchor({ top: 50, bottom: 80, left: 10, right: 40 });
    expect(computeAnchorPosition(nearEdge, 100, 200, null, 'center').left).toBe(8);
  });
});

describe('pointAnchor', () => {
  it('opens a panel just below and right of the pointer', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(pointAnchor({ x: 200, y: 300 }), 200, 240);
    expect(pos.placement).toBe('bottom-start');
    expect(pos.left).toBe(200);
    expect(pos.top).toBe(304);
  });

  it('flips above the pointer near the bottom edge', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(pointAnchor({ x: 200, y: 700 }), 200, 240);
    expect(pos.placement).toBe('top-start');
    expect(pos.top).toBe(496);
  });

  it('clamps inside the viewport near the right edge', () => {
    setViewport(1280, 800);
    const pos = computeAnchorPosition(pointAnchor({ x: 1270, y: 300 }), 200, 240);
    expect(pos.left).toBe(1032);
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

  it('treats a tap on a toast as not outside, text nodes included', () => {
    const panel = elWith();
    const anchor = elWith();
    const button = toastButton();
    const label = { nodeType: 3, parentElement: button } as unknown as Node;
    expect(isOutsidePointerTarget(button, panel, anchor)).toBe(false);
    expect(isOutsidePointerTarget(label, panel, anchor)).toBe(false);
    expect(isOutsidePointerTarget(fakeElement(false), panel, anchor)).toBe(true);
  });

  it('closes a backdrop-less overlay on a toast tap without swallowing it', () => {
    const button = toastButton();
    const onDismiss = vi.fn();
    const onArm = vi.fn();
    const h = makeDismissHandlers({ current: elWith() }, elWith(), onDismiss, onArm, () => true, false, true);
    h.onPointerDown({ target: button, button: 0, isPrimary: true } as unknown as PointerEvent);
    const touch = { target: button, stopPropagation: vi.fn(), preventDefault: vi.fn() };
    h.onTouchEnd(touch as unknown as TouchEvent);
    const click = { target: button, stopPropagation: vi.fn(), preventDefault: vi.fn() };
    h.onClickCapture(click as unknown as MouseEvent);
    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(onArm).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('leaves a lower overlay open when a toast tap light-dismisses', () => {
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: elWith() }, elWith(), onDismiss, undefined, () => false, false, true);
    h.onPointerDown({ target: toastButton(), button: 0, isPrimary: true } as unknown as PointerEvent);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('keeps a backdrop modal open on a toast tap', () => {
    const button = toastButton();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: elWith() }, elWith(), onDismiss);
    h.onPointerDown({ target: button, button: 0, isPrimary: true } as unknown as PointerEvent);
    const click = { target: button, stopPropagation: vi.fn(), preventDefault: vi.fn() };
    h.onClickCapture(click as unknown as MouseEvent);
    expect(onDismiss).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });
});


function pointerDownAt(target: Node, button = 0): PointerEvent {
  return { target, type: 'pointerdown', button } as unknown as PointerEvent;
}

function clickEvent(target?: Node): MouseEvent {
  return {
    type: 'click',
    target,
    stopPropagation: vi.fn(),
    preventDefault: vi.fn(),
  } as unknown as MouseEvent;
}

function touchEndAt(target: Node): TouchEvent {
  return {
    type: 'touchend',
    target,
    stopPropagation: vi.fn(),
    preventDefault: vi.fn(),
  } as unknown as TouchEvent;
}

describe('makeDismissHandlers', () => {
  it('dismisses and swallows the paired click when pointerdown lands outside panel + anchor', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const click = clickEvent();
    h.onClickCapture(click);
    expect(click.stopPropagation).toHaveBeenCalledTimes(1);
    expect(click.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('only swallows ONE click per outside-pointerdown — a subsequent click ON the panel passes through untouched', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    const first = clickEvent(elsewhere);
    h.onClickCapture(first);
    expect(first.stopPropagation).toHaveBeenCalledTimes(1);

    const second = clickEvent(panel);
    h.onClickCapture(second);
    expect(second.stopPropagation).not.toHaveBeenCalled();
    expect(second.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT dismiss or swallow when pointerdown lands inside the panel', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(panel));
    expect(onDismiss).not.toHaveBeenCalled();

    const click = clickEvent(panel);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT swallow the click when pointerdown lands on the anchor — re-clicking the anchor must toggle the popover', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(anchor));
    expect(onDismiss).not.toHaveBeenCalled();

    const click = clickEvent(anchor);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('Escape dismisses (no click-swallow side effect on an inside click)', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onKey({ key: 'Escape' } as KeyboardEvent);
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const click = clickEvent(panel);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
  });

  it('non-Escape keys do nothing', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onKey({ key: 'Enter' } as KeyboardEvent);
    h.onKey({ key: 'a' } as KeyboardEvent);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('right-click pointerdown outside dismisses but does NOT arm the suppressor — a later click ON the panel must pass through', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere, 2));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const laterClick = clickEvent(panel);
    h.onClickCapture(laterClick);
    expect(laterClick.stopPropagation).not.toHaveBeenCalled();
    expect(laterClick.preventDefault).not.toHaveBeenCalled();
  });

  it('middle-click pointerdown outside dismisses but does NOT arm the suppressor', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere, 1));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const laterClick = clickEvent(panel);
    h.onClickCapture(laterClick);
    expect(laterClick.stopPropagation).not.toHaveBeenCalled();
  });

  it('reads panelRef.current lazily — a ref filled in after construction is still respected', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const ref: { current: HTMLElement | null } = { current: null };
    const h = makeDismissHandlers(ref, anchor, onDismiss);

    ref.current = panel;

    h.onPointerDown(pointerDownAt(panel));
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('synthetic click without a preceding pointerdown still dismisses + swallows (fallback path)', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const click = clickEvent(elsewhere);
    h.onClickCapture(click);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(click.stopPropagation).toHaveBeenCalledTimes(1);
    expect(click.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('a press that began INSIDE the panel never dismisses, wherever its click lands', () => {
    const panel = elWith();
    const anchor = elWith();
    const backdrop = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(panel));
    const click = clickEvent(backdrop);
    h.onClickCapture(click);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();

    h.onClickCapture(clickEvent(backdrop));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it('synthetic click INSIDE the panel does not dismiss', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const click = clickEvent(panel);
    h.onClickCapture(click);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
  });

  it('synthetic click on the anchor does not dismiss (the anchor toggles via its own handler)', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const click = clickEvent(anchor);
    h.onClickCapture(click);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(click.stopPropagation).not.toHaveBeenCalled();
  });

  it('a click the INSIDE click dispatches is not an outside click (the attach menu File item)', async () => {
    const panel = elWith();
    const anchor = elWith();
    const hiddenFileInput = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onClickCapture(clickEvent(panel));
    await Promise.resolve();
    const nested = clickEvent(hiddenFileInput);
    h.onClickCapture(nested);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(nested.stopPropagation).not.toHaveBeenCalled();
    expect(nested.preventDefault).not.toHaveBeenCalled();
  });

  it('the window closes when the inside click finishes, so an unrelated outside click in the SAME task is still swallowed', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const inside = clickEvent(panel);
    h.onClickCapture(inside);
    h.onClickBubble(inside);

    const later = clickEvent(elsewhere);
    h.onClickCapture(later);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(later.stopPropagation).toHaveBeenCalledTimes(1);
    expect(later.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('a nested click bubbling back up does NOT close the window, so a second nested click is also let through', () => {
    const panel = elWith();
    const anchor = elWith();
    const first = elWith();
    const second = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const inside = clickEvent(panel);
    h.onClickCapture(inside);

    const nestedA = clickEvent(first);
    h.onClickCapture(nestedA);
    h.onClickBubble(nestedA);
    const nestedB = clickEvent(second);
    h.onClickCapture(nestedB);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(nestedB.preventDefault).not.toHaveBeenCalled();
  });

  it('the window is backstopped by a task, for an inside click whose handler stops propagation', async () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onClickCapture(clickEvent(panel));
    await new Promise((resolve) => setTimeout(resolve, 0));

    const later = clickEvent(elsewhere);
    h.onClickCapture(later);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(later.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('onDismiss returning false on the click-only fallback path skips the swallow', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn(() => false as const);
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const click = clickEvent(elsewhere);
    h.onClickCapture(click);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('onDismiss returning false skips the click suppressor — neighbor click passes through', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn(() => false as const);
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const click = clickEvent(elsewhere);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });


  it('swallows an outside touch-driven action: outside pointerdown then outside touchend is stopped + prevented', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    const touch = touchEndAt(elsewhere);
    h.onTouchEnd(touch);
    expect(touch.stopPropagation).toHaveBeenCalledTimes(1);
    expect(touch.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('touchend disarms the suppressor so a (hypothetical) later click is not double-swallowed', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    h.onTouchEnd(touchEndAt(elsewhere));

    const click = clickEvent(panel);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT swallow a touchend on the anchor — the toggle must close via its own handler', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(anchor));
    expect(onDismiss).not.toHaveBeenCalled();

    const touch = touchEndAt(anchor);
    h.onTouchEnd(touch);
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT swallow a touchend inside the panel', () => {
    const panel = elWith();
    const anchor = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(panel));
    const touch = touchEndAt(panel);
    h.onTouchEnd(touch);
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT swallow a touchend on the anchor even when an outside pointerdown armed the suppressor (target guard)', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    const touch = touchEndAt(anchor);
    h.onTouchEnd(touch);
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('a bare touchend with no preceding outside pointerdown is not swallowed', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const touch = touchEndAt(elsewhere);
    h.onTouchEnd(touch);
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });


  it('calls onArm on an outside primary pointerdown that dismissed', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onArm = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, vi.fn(), onArm);

    h.onPointerDown(pointerDownAt(elsewhere));
    expect(onArm).toHaveBeenCalledTimes(1);
  });

  it('does NOT call onArm for inside / anchor / right-click / no-op dismiss', () => {
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();

    const armInside = vi.fn();
    makeDismissHandlers({ current: panel }, anchor, vi.fn(), armInside).onPointerDown(pointerDownAt(panel));
    expect(armInside).not.toHaveBeenCalled();

    const armAnchor = vi.fn();
    makeDismissHandlers({ current: panel }, anchor, vi.fn(), armAnchor).onPointerDown(pointerDownAt(anchor));
    expect(armAnchor).not.toHaveBeenCalled();

    const armRight = vi.fn();
    makeDismissHandlers({ current: panel }, anchor, vi.fn(), armRight).onPointerDown(pointerDownAt(elsewhere, 2));
    expect(armRight).not.toHaveBeenCalled();

    const armNoop = vi.fn();
    makeDismissHandlers({ current: panel }, anchor, () => false as const, armNoop).onPointerDown(pointerDownAt(elsewhere));
    expect(armNoop).not.toHaveBeenCalled();
  });

  describe('when another overlay is stacked on top', () => {
    it('does not dismiss on a pointerdown meant for the overlay above', () => {
      const panel = elWith();
      const upperPanel = elWith();
      const onDismiss = vi.fn();
      const arm = vi.fn();
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, arm, () => false);

      h.onPointerDown(pointerDownAt(upperPanel));
      h.onPointerDown(pointerDownAt(elWith()));
      expect(onDismiss).not.toHaveBeenCalled();
      expect(arm).not.toHaveBeenCalled();
    });

    it('does not dismiss on a synthetic click either', () => {
      const onDismiss = vi.fn();
      const h = makeDismissHandlers({ current: elWith() }, null, onDismiss, undefined, () => false);
      const click = clickEvent(elWith());
      h.onClickCapture(click);
      expect(onDismiss).not.toHaveBeenCalled();
      expect(click.preventDefault).not.toHaveBeenCalled();
    });

    it('does not answer Escape', () => {
      const onDismiss = vi.fn();
      const h = makeDismissHandlers({ current: elWith() }, null, onDismiss, undefined, () => false);
      h.onKey({ key: 'Escape' } as KeyboardEvent);
      expect(onDismiss).not.toHaveBeenCalled();
    });

    it('still dismisses the overlay that IS on top', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => true);
      h.onPointerDown(pointerDownAt(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it('ignores the paired click of a pointerdown the overlay above consumed', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      let top = false;
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => top);

      const elsewhere = elWith();
      h.onPointerDown(pointerDownAt(elsewhere));
      top = true;
      const click = clickEvent(elsewhere);
      h.onClickCapture(click);

      expect(onDismiss).not.toHaveBeenCalled();
      expect(click.preventDefault).not.toHaveBeenCalled();
    });

    it('still answers a synthetic click once the gesture is spent', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      let top = false;
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => top);

      h.onPointerDown(pointerDownAt(elWith()));
      top = true;
      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).not.toHaveBeenCalled();

      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it.each([
      ['a secondary button, which dispatches contextmenu and no click', (h: ReturnType<typeof makeDismissHandlers>, t: Node) => h.onPointerDown(pointerDownAt(t, 2))],
      ['a cancelled gesture, taken over by a scroll or a drag', (h: ReturnType<typeof makeDismissHandlers>, t: Node) => { h.onPointerDown(pointerDownAt(t)); h.onCancel(); }],
    ])('does not strand the pairing after %s', (_name, gesture) => {
      const onDismiss = vi.fn();
      let top = false;
      const h = makeDismissHandlers({ current: elWith() }, null, onDismiss, undefined, () => top);

      gesture(h, elWith());
      top = true;
      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it('clears the pairing on a touch that produces no click', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      let top = false;
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => top);

      const elsewhere = elWith();
      h.onPointerDown(pointerDownAt(elsewhere));
      h.onTouchEnd(touchEndAt(elsewhere));
      top = true;

      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });
  });

  describe('when a press was already down as the overlay opened', () => {
    const openedUnderPress = (panel: HTMLElement, onDismiss: () => void) =>
      makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => true, true);

    it('spends the opening gesture click instead of dismissing on it', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      const click = clickEvent(elWith());
      h.onClickCapture(click);
      expect(onDismiss).not.toHaveBeenCalled();
      expect(click.stopPropagation).not.toHaveBeenCalled();
      expect(click.preventDefault).not.toHaveBeenCalled();
    });

    it('dismisses on the NEXT outside click, the opening one being spent', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      h.onClickCapture(clickEvent(elWith()));
      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it('keeps the expectation across the touchend that ends the press', () => {
      const panel = elWith();
      const row = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      h.onTouchEnd(touchEndAt(row));
      h.onClickCapture(clickEvent(row));
      expect(onDismiss).not.toHaveBeenCalled();
    });

    it('drops the expectation when a NEW press starts', () => {
      const panel = elWith();
      const elsewhere = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      h.onPointerDown(pointerDownAt(elsewhere));
      expect(onDismiss).toHaveBeenCalledTimes(1);
      const click = clickEvent(elsewhere);
      h.onClickCapture(click);
      expect(click.stopPropagation).toHaveBeenCalledTimes(1);
    });

    it('drops the expectation when the gesture is cancelled', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      h.onCancel();
      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    it('leaves a click INSIDE the panel alone, as always', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = openedUnderPress(panel, onDismiss);

      const click = clickEvent(panel);
      h.onClickCapture(click);
      expect(onDismiss).not.toHaveBeenCalled();
      expect(click.preventDefault).not.toHaveBeenCalled();
    });
  });

  it('an overlay opened with NO press down still dismisses on a synthetic click', () => {
    const panel = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => true, false);

    h.onClickCapture(clickEvent(elWith()));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});

describe('installPairedSwallow', () => {
  function dispatch(type: string): { stopPropagation: ReturnType<typeof vi.fn>; preventDefault: ReturnType<typeof vi.fn> } {
    const e = { type, stopPropagation: vi.fn(), preventDefault: vi.fn() };
    (document as unknown as { dispatchEvent: (e: unknown) => boolean }).dispatchEvent(e);
    return e;
  }

  it('swallows the next touchend, then removes itself (one-shot)', () => {
    installPairedSwallow();
    const first = dispatch('touchend');
    expect(first.stopPropagation).toHaveBeenCalledTimes(1);
    expect(first.preventDefault).toHaveBeenCalledTimes(1);

    const second = dispatch('touchend');
    expect(second.stopPropagation).not.toHaveBeenCalled();
    expect(second.preventDefault).not.toHaveBeenCalled();
  });

  it('swallows the next click (mouse case), then removes itself', () => {
    installPairedSwallow();
    const click = dispatch('click');
    expect(click.stopPropagation).toHaveBeenCalledTimes(1);
    expect(click.preventDefault).toHaveBeenCalledTimes(1);

    const later = dispatch('click');
    expect(later.stopPropagation).not.toHaveBeenCalled();
  });

  it('after a touchend swallow, a follow-up click is NOT also swallowed', () => {
    installPairedSwallow();
    dispatch('touchend');
    const click = dispatch('click');
    expect(click.stopPropagation).not.toHaveBeenCalled();
  });

  it('a touchcancel tears the swallow down without eating a later tap', () => {
    installPairedSwallow();
    dispatch('touchcancel');
    const touch = dispatch('touchend');
    expect(touch.stopPropagation).not.toHaveBeenCalled();
  });

  it.each(['touchcancel', 'pointercancel'])('a %s names the swallow, so no button serves the press', (type) => {
    takePressOutcome(1000);
    installPairedSwallow();
    const cancel = dispatch(type);
    expect(takePressOutcome(1000)).toBe('swallowed');
    expect(cancel.stopPropagation).not.toHaveBeenCalled();
  });

  it('a new gesture tears down an arm whose paired event never arrived', () => {
    installPairedSwallow();
    dispatch('pointerdown');
    const touch = dispatch('touchend');
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('the arming pointerdown itself does not tear the arm down', () => {
    const arming = { type: 'pointerdown', stopPropagation: vi.fn(), preventDefault: vi.fn() };
    installPairedSwallow(arming as unknown as Event);
    (document as unknown as { dispatchEvent: (e: unknown) => boolean }).dispatchEvent(arming);
    const touch = dispatch('touchend');
    expect(touch.stopPropagation).toHaveBeenCalledTimes(1);
  });

  it('names the swallow, so an observer cannot read it as a dead press', () => {
    takePressOutcome(1000);
    installPairedSwallow();
    dispatch('touchend');
    expect(takePressOutcome(1000)).toBe('swallowed');
  });
});

describe('takePressOutcome', () => {
  it('is null when nobody claimed the press', () => {
    takePressOutcome(1000);
    expect(takePressOutcome(1000)).toBeNull();
  });

  it('consumes, so one press cannot describe the next', () => {
    notePressOutcome('served', 100);
    expect(takePressOutcome(1000, 100)).toBe('served');
    expect(takePressOutcome(1000, 100)).toBeNull();
  });

  it('forgets an outcome older than the window', () => {
    notePressOutcome('served', 100);
    expect(takePressOutcome(500, 900)).toBeNull();
  });

  it('a window measured from a press start excludes an EARLIER press', () => {
    notePressOutcome('served', 100);
    const armedAt = 200;
    const now = 800;
    expect(takePressOutcome(now - armedAt, now)).toBeNull();
  });

  it('a window measured from a press start keeps that press own claim', () => {
    const armedAt = 200;
    notePressOutcome('swallowed', 250);
    const now = 800;
    expect(takePressOutcome(now - armedAt, now)).toBe('swallowed');
  });
});
