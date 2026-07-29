import { describe, it, expect, vi } from 'vitest';
import { computeAnchorPosition, isOutsidePointerTarget, makeDismissHandlers, installPairedSwallow } from './useAnchoredPopover';

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

  it("align 'end' aligns the panel's right edge with the anchor's right edge", () => {
    setViewport(1280, 800);
    // Anchor (⋯ trigger) on the right; a 300px menu should hang its right edge
    // under the trigger's right edge: 1040 - 300 = 740.
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 1010, right: 1040 }),
      200,
      300,
      null,
      'end',
    );
    expect(pos.left).toBe(740);
    expect(pos.left + 300).toBe(1040); // right edge pinned to the anchor
  });

  it("align 'end' stays anchored to the trigger on a narrow mobile viewport (the bug fix)", () => {
    setViewport(393, 852); // iPhone 14 Pro
    const anchor = fakeAnchor({ top: 100, bottom: 120, left: 350, right: 380 });
    // 'start' clamps a 320px menu to 393 - 320 - 8 = 65 — detached from the
    // trigger near the left edge. 'end' pins its right edge under the trigger.
    const start = computeAnchorPosition(anchor, 200, 320);
    const end = computeAnchorPosition(anchor, 200, 320, null, 'end');
    expect(start.left).toBe(65);
    expect(end.left).toBe(60); // 380 - 320
    expect(end.left + 320).toBe(380); // right edge under the trigger
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
    // desiredLeft = 350 - 380 = -30; clamp pins to the left margin.
    expect(pos.left).toBe(8);
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

// ──────────────────────────────────────────────────────────────────────────
// makeDismissHandlers — the canonical Lucidos modal contract: outside
// pointerdown dismisses AND swallows the paired click so the underlying
// element (a sibling button, a chat row, etc.) does NOT also fire.
// See .claude/rules/frontend.md § "Modals & popovers: click-outside dismiss".
// ──────────────────────────────────────────────────────────────────────────

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
    // The suppressor is one-shot: a pointerdown+click pair consumes it, and a
    // *later* click on the panel itself (the inside-target path) must reach
    // its own handler. (A later click OUTSIDE the panel goes through the
    // synthetic-click fallback and gets dismissed — covered by the dedicated
    // fallback test.)
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

    // Escape didn't arm the click suppressor — a click on the panel should
    // pass through. (Outside clicks go through the fallback path and get
    // swallowed; see the dedicated fallback tests.)
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
    // Regression test: right-click dispatches `contextmenu`, not `click`, so
    // arming the suppressor on right-click would strand the flag and swallow
    // a subsequent click on the panel itself.
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere, 2));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    // A later click ON the panel (inside-target path) must NOT be swallowed —
    // the suppressor was not armed by the right-click.
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

    // Ref filled in after the handlers are built (mirrors React/Preact ref timing).
    ref.current = panel;

    h.onPointerDown(pointerDownAt(panel));
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it('synthetic click without a preceding pointerdown still dismisses + swallows (fallback path)', () => {
    // Regression: HTMLElement.click() fires `click` only — no pointerdown. The
    // previous hand-rolled handler used document click-capture and dismissed
    // either way; the canonical hook lost that path during the refactor and
    // broke e2e tests that drive dismiss via synthetic clicks (and any
    // keyboard-shortcut / programmatic-click code path). Click-capture mirrors
    // the pointerdown path's dismiss-and-swallow behaviour, but only when the
    // suppressor was NOT already armed (real user gestures route through
    // pointerdown first and rely on the armed-suppressor path).
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

  it('onDismiss returning false on the click-only fallback path skips the swallow', () => {
    // Mirror of the pointerdown-side test: a no-op dismiss (e.g. Drawer
    // already mid-close) must not eat the synthetic click either.
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
    // Regression: Drawer's close animation runs 200ms. During the animation,
    // closeDrawer() is a no-op (drawerClosing already true) but the dismiss
    // hook still arms the suppressor on outside pointerdown, swallowing the
    // user's tap on a sibling button (file-search-btn, content actions, …).
    // Returning false from onDismiss says "I had nothing to do here — let the
    // paired click reach its real handler."
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

  // ── Touch path ──────────────────────────────────────────────────────────
  // A button can run its action on `touchend` and `preventDefault()` the
  // synthetic click (the iOS keyboard-nudge pattern in `composeHandlers`). On
  // touch the dismiss contract must swallow that `touchend` too — otherwise the
  // outside pointerdown dismisses the overlay AND the button still fires its
  // action on the same tap (the reported compose-on-first-tap bug), and because
  // the button preventDefaults the synthetic click, the click never arrives to
  // disarm the suppressor (the stranded-flag second bug).

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
    // touchend.preventDefault cancels the synthetic click on touch, but if any
    // click DID arrive afterwards the suppressor must already be consumed — the
    // touchend owns the disarm. Guards the stranded-flag regression.
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere));
    h.onTouchEnd(touchEndAt(elsewhere));

    // A subsequent click ON the panel must pass through — the suppressor was
    // already disarmed by the touchend, not left stranded.
    const click = clickEvent(panel);
    h.onClickCapture(click);
    expect(click.stopPropagation).not.toHaveBeenCalled();
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  it('does NOT swallow a touchend on the anchor — the toggle must close via its own handler', () => {
    // Real anchor tap: pointerdown on the anchor is exempt, so the suppressor
    // never arms; the touchend then passes through to the anchor's own toggle.
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
    // Contrived finger-move (outside pointerdown → anchor touchend): the target
    // guard keeps the anchor exempt so its own handler still toggles.
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onPointerDown(pointerDownAt(elsewhere)); // arms the suppressor
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

  // ── onArm: the overlay-outliving swallow ──────────────────────────────────
  // The dismiss closes the overlay, whose re-render tears down these handlers
  // before the paired touchend/click fires. So an outside-primary pointerdown
  // also calls onArm() to install a swallow that survives the unmount.

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

    // onDismiss returning false (e.g. Drawer mid-close) → no arm, so the
    // surviving swallow can't eat the user's tap on a sibling button.
    const armNoop = vi.fn();
    makeDismissHandlers({ current: panel }, anchor, () => false as const, armNoop).onPointerDown(pointerDownAt(elsewhere));
    expect(armNoop).not.toHaveBeenCalled();
  });
});

// ──────────────────────────────────────────────────────────────────────────
// installPairedSwallow — the overlay-outliving one-shot. An outside-primary
// pointerdown dismiss re-renders and removes the overlay's own listeners before
// the gesture's paired touchend/click fires, so the swallow lives on document
// listeners that aren't tied to the overlay's mount. Uses the document stub from
// test-setup.ts (add/remove/dispatch).
// ──────────────────────────────────────────────────────────────────────────
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

    // A later touchend is no longer swallowed — the one-shot tore itself down.
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
    // Touch fires touchend then (preventDefaulted) would-be click; the one-shot
    // is consumed by the touchend, so any click that slips through passes.
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
});
