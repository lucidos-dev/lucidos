import { describe, it, expect, vi } from 'vitest';
import { computeAnchorPosition, isOutsidePointerTarget, makeDismissHandlers, installPairedSwallow } from './useAnchoredPopover';
import { notePressOutcome, takePressOutcome } from '../utils/tapGesture';

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

  /** The prompt-bar popover regression, in its own shape. Both prompt-bar
   *  indicators (subscription clock, todo list) sit near the LEFT of the
   *  actions row, so the anchor is nowhere near the right edge and the panel
   *  still cannot start at the anchor: it is nearly as wide as the phone. The
   *  shell used to place these with `position: absolute; left: 0`, which has no
   *  viewport to clamp against, and the panel's own countdown fell off the
   *  right edge. Pins that a left-ish anchor with a wide panel is pulled BACK
   *  toward the left margin, not left where it sits. */
  it('pulls a wide panel back from a left-of-centre anchor on a phone', () => {
    setViewport(393, 852); // iPhone 14 Pro
    const clockButton = fakeAnchor({ top: 760, bottom: 796, left: 78, right: 114 });
    const pos = computeAnchorPosition(clockButton, 300, 375);
    // Unclamped this is left: 78, so the 375px panel would end at 453 on a
    // 393px screen. Clamped: 393 - 375 - 8 = 10.
    expect(pos.left).toBe(10);
    // No room below a bottom-docked prompt bar, so it opens upward.
    expect(pos.placement).toBe('top-start');
  });

  /** The flip has the same off-screen failure the horizontal clamp exists for,
   *  one axis over. A tall panel above a bottom-docked anchor on a SHORT
   *  viewport (landscape, or the keyboard shrinking the visual viewport) gets a
   *  negative `top`, and unlike a horizontal overflow the user cannot scroll it
   *  back: the panel's own scroll container has already moved off-screen. */
  it('keeps a flipped panel on screen when there is not enough room above', () => {
    setViewport(393, 400); // landscape-ish, keyboard open
    const clockButton = fakeAnchor({ top: 300, bottom: 336, left: 78, right: 114 });
    const pos = computeAnchorPosition(clockButton, 324, 320);
    // Unclamped this is 300 - 324 - 4 = -28, with the panel's head off the top.
    expect(pos.top).toBe(8);
    expect(pos.placement).toBe('top-start');
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

  /** Clamping only helps a panel that FITS: a wider one pins to the leading
   *  edge and overflows the far one. `maxWidth` is what a surface caps itself
   *  with so that never happens, and it must describe the CONTAINER, not the
   *  viewport, or a thread-pane popover on desktop is capped against a box
   *  twice its size and half of it lands in the content pane. */
  it('reports the container width, margins deducted, as the panel fit', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 500 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 80, right: 130 }),
      200,
      300,
      container,
    );
    expect(pos.maxWidth).toBe(484); // 500 - 2 * 8
  });

  it('falls back to the viewport width for the fit when no container is given', () => {
    setViewport(393, 852);
    const pos = computeAnchorPosition(fakeAnchor({ top: 100, bottom: 120, left: 78, right: 114 }), 200, 300);
    expect(pos.maxWidth).toBe(377); // 393 - 2 * 8
  });

  /** A collapsed pane (the maximize shortcut, fired while a popover is open)
   *  is a zero-width container. Capping to it would render a zero-width panel:
   *  invisible, while the overlay is still open and the UI behind it inert. */
  it('falls back to the viewport when the container has collapsed to nothing', () => {
    setViewport(1280, 800);
    const collapsed = fakeAnchor({ top: 0, bottom: 800, left: 0, right: 0 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 0, right: 0 }),
      200,
      300,
      collapsed,
    );
    expect(pos.maxWidth).toBe(1264); // 1280 - 2 * 8, not 0
  });

  /** The first measurement necessarily runs on the UNCAPPED panel, since the
   *  cap is what this call produces. It still lands right: an over-wide panel
   *  pins to the container's leading edge, which is exactly where it belongs
   *  once it narrows to the reported fit. */
  it('pins an over-wide panel where the capped panel will sit', () => {
    setViewport(1280, 800);
    const container = fakeAnchor({ top: 0, bottom: 800, left: 200, right: 700 }) as HTMLElement;
    const pos = computeAnchorPosition(
      fakeAnchor({ top: 100, bottom: 120, left: 240, right: 276 }),
      200,
      900, // wider than the 500px pane it opened in
      container,
    );
    expect(pos.left).toBe(208); // container left + margin
    expect(pos.maxWidth).toBe(484);
    expect(pos.left + pos.maxWidth).toBe(692); // container right - margin
  });

  /** Why `useAnchoredPosition` watches the panel's own size (a ResizeObserver
   *  feeding the same rAF-coalesced recompute), not just the viewport's.
   *  Applying the fit above NARROWS the panel, which reflows its text onto more
   *  lines and makes it TALLER, and for an upward-opening panel the height is
   *  what `top` is derived from. Measured once at the uncapped height, the
   *  panel would hang down over the prompt-bar button that opened it. */
  it('re-derives top from the panel height, so a reflowed panel clears its anchor', () => {
    setViewport(1280, 900);
    const clockButton = fakeAnchor({ top: 800, bottom: 836, left: 300, right: 336 });
    const container = fakeAnchor({ top: 0, bottom: 900, left: 200, right: 700 }) as HTMLElement;
    const uncapped = computeAnchorPosition(clockButton, 120, 900, container);
    const reflowed = computeAnchorPosition(clockButton, 220, 484, container);
    expect(uncapped.placement).toBe('top-start');
    expect(reflowed.placement).toBe('top-start');
    // Held at the stale `top`, the taller panel would end 100px past the
    // anchor's top edge; re-measured, its bottom sits just above the anchor.
    expect(uncapped.top + 220).toBeGreaterThan(clockButton.getBoundingClientRect().top);
    expect(reflowed.top + 220).toBeLessThanOrEqual(clockButton.getBoundingClientRect().top);
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

  it('a click the INSIDE click dispatches is not an outside click (the attach menu File item)', async () => {
    // Regression: the composer's attach menu closes itself and calls .click()
    // on the persistent hidden <input type="file">, which lives OUTSIDE the
    // panel (inside .prompt-box, so the menu's re-render can't unmount it
    // mid-tap). That nested click reached the fallback below, which
    // preventDefault()'d it, and showing the file chooser is the cancelable
    // DEFAULT ACTION of a click on a file input, so the menu item did nothing
    // at all, silently. A click dispatched while an inside click is still
    // unwinding is a consequence of that click, never a new outside one.
    const panel = elWith();
    const anchor = elWith();
    const hiddenFileInput = elWith(); // outside the panel, like the real one
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    // Document capture sees the menu item's click first, then the item's own
    // bubble handler runs and dispatches input.click(), whose click re-enters
    // the same capture listener before the outer dispatch has unwound.
    //
    // The await between them is NOT ceremony: the browser runs a microtask
    // checkpoint every time the JS stack empties, so one runs between two
    // listeners of a single dispatch. Without it this test passes against a
    // guard cleared on a microtask, which is exactly the version that shipped
    // green here and dead in Chromium.
    h.onClickCapture(clickEvent(panel));
    await Promise.resolve();
    const nested = clickEvent(hiddenFileInput);
    h.onClickCapture(nested);

    expect(onDismiss).not.toHaveBeenCalled();
    expect(nested.stopPropagation).not.toHaveBeenCalled();
    expect(nested.preventDefault).not.toHaveBeenCalled();
  });

  it('the window closes when the inside click finishes, so an unrelated outside click in the SAME task is still swallowed', () => {
    // The other half of the guard: it must not become "one free outside click
    // for the rest of the task", which would break the fallback for anything
    // clicking twice in one go. The inside click's own bubble to document is
    // the end of its dispatch, so nothing after it is nested in it.
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    const inside = clickEvent(panel);
    h.onClickCapture(inside);
    h.onClickBubble(inside); // the dispatch ends here, no timer involved

    const later = clickEvent(elsewhere);
    h.onClickCapture(later);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(later.stopPropagation).toHaveBeenCalledTimes(1);
    expect(later.preventDefault).toHaveBeenCalledTimes(1);
  });

  it('a nested click bubbling back up does NOT close the window, so a second nested click is also let through', () => {
    // The nested click reaches document in the bubble phase too, before the
    // outer one does. Matching on the event object is what keeps that from
    // closing the window early on a handler that clicks two elements.
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
    // A target handler that calls stopPropagation() means the bubble never
    // reaches document, so the event-object match alone would strand the
    // window open for the life of the overlay.
    const panel = elWith();
    const anchor = elWith();
    const elsewhere = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, anchor, onDismiss);

    h.onClickCapture(clickEvent(panel)); // no matching onClickBubble: swallowed
    await new Promise((resolve) => setTimeout(resolve, 0));

    const later = clickEvent(elsewhere);
    h.onClickCapture(later);

    expect(onDismiss).toHaveBeenCalledTimes(1);
    expect(later.preventDefault).toHaveBeenCalledTimes(1);
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

  // ──────────────────────────────────────────────────────────────────────
  // Stacked overlays. Each open overlay installs its own document listener,
  // and each asks only whether the target sits outside ITS panel. A SIBLING
  // overlay's panel is outside, so the lower one dismissed itself invisibly
  // behind whatever the user had just opened. `isTop` is the gate.
  //
  // A dropdown nested INSIDE a modal never showed this, because the modal's
  // panel contains it. The pair that does is the event-wait condition modal
  // opened from the waiting panel, which are siblings in the overlay layer.
  // ──────────────────────────────────────────────────────────────────────
  describe('when another overlay is stacked on top', () => {
    it('does not dismiss on a pointerdown meant for the overlay above', () => {
      const panel = elWith();
      const upperPanel = elWith();
      const onDismiss = vi.fn();
      const arm = vi.fn();
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, arm, () => false);

      // A click landing on the upper overlay's panel, and one on its scrim.
      h.onPointerDown(pointerDownAt(upperPanel));
      h.onPointerDown(pointerDownAt(elWith()));
      expect(onDismiss).not.toHaveBeenCalled();
      // Nor may it arm the swallow: the top overlay owns that half too, and
      // two armed one-shots would eat a later unrelated tap.
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

    /** Escape is dispatched LIFO by `useKeyboardShortcuts` against the same
     *  stack, so this handler is the fallback. It must agree. */
    it('does not answer Escape', () => {
      const onDismiss = vi.fn();
      const h = makeDismissHandlers({ current: elWith() }, null, onDismiss, undefined, () => false);
      h.onKey({ key: 'Escape' } as KeyboardEvent);
      expect(onDismiss).not.toHaveBeenCalled();
    });

    /** The top overlay is unaffected, which is what keeps the single-overlay
     *  case (every other caller in the app) exactly as it was. */
    it('still dismisses the overlay that IS on top', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => true);
      h.onPointerDown(pointerDownAt(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    /** **The gate alone was not enough, because a mouse gesture is two events
     *  in two tasks.** The upper overlay consumes the pointerdown and unmounts
     *  on the microtask between them. So by the time the paired click lands,
     *  the lower overlay IS the top panel. It read that click as unpaired,
     *  took the synthetic-click fallback, and closed anyway, one task after
     *  the gate had correctly held it shut.
     *
     *  Touch never showed it: the swallow's `preventDefault` on `touchend`
     *  cancels the synthetic click, so the fallback never runs. */
    it('ignores the paired click of a pointerdown the overlay above consumed', () => {
      const panel = elWith();
      const onDismiss = vi.fn();
      // Not top while the pointerdown lands, top by the time its click does.
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

    /** The flag is one gesture wide. A genuinely synthetic click arriving
     *  later, with no pointerdown of its own, still dismisses. */
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

    /** **A pairing must never outlive the gesture that announced it.** Stranded
     *  set, it eats the next synthetic click's dismiss. That click then neither
     *  closes this overlay nor is swallowed, so it activates whatever sits
     *  under it. Two gestures end without the click they promised. */
    it.each([
      ['a secondary button, which dispatches contextmenu and no click', (h: ReturnType<typeof makeDismissHandlers>, t: Node) => h.onPointerDown(pointerDownAt(t, 2))],
      ['a cancelled gesture, taken over by a scroll or a drag', (h: ReturnType<typeof makeDismissHandlers>, t: Node) => { h.onPointerDown(pointerDownAt(t)); h.onCancel(); }],
    ])('does not strand the pairing after %s', (_name, gesture) => {
      const onDismiss = vi.fn();
      let top = false;
      const h = makeDismissHandlers({ current: elWith() }, null, onDismiss, undefined, () => top);

      gesture(h, elWith());
      top = true;
      // A synthetic click with no pointerdown of its own must still dismiss.
      h.onClickCapture(clickEvent(elWith()));
      expect(onDismiss).toHaveBeenCalledTimes(1);
    });

    /** Touch ends the gesture at `touchend`, since the swallow cancels the
     *  click that would otherwise clear the flag. */
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

  /** A gesture-opened overlay: a long press on a mobile drawer row opens its
   *  actions menu, and the menu has no anchor to exempt. The lift ending that
   *  press dispatches a click these handlers saw no pointerdown for. The
   *  fallback read it as synthetic and dismissed the menu the hold had just
   *  opened. */
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
      // `onTouchEnd` clears the ordinary pairing, because a dismissing tap's
      // click is cancelled by the swallow. This gesture's click is not.
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

      // A second gesture: its own pointerdown dismisses, and the click it
      // pairs with is swallowed rather than read as the opening one.
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
    // The canary the fix must not break: `e2e/overlay-dismiss-swallow.spec.ts`
    // opens the same menu with dispatched (untrusted) pointer events, which
    // the browser pairs no click with, and then dismisses it synthetically.
    const panel = elWith();
    const onDismiss = vi.fn();
    const h = makeDismissHandlers({ current: panel }, null, onDismiss, undefined, () => true, false);

    h.onClickCapture(clickEvent(elWith()));
    expect(onDismiss).toHaveBeenCalledTimes(1);
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

  // The arm belongs to ONE gesture. Its paired event can strand: a touchend
  // dispatched to a node the dismiss REMOVED never reaches document, and no
  // cancel fires. A stranded arm that survives eats the user's next tap, which
  // is a dead button (docs/plans/2026-08-28-a-swallowed-tap-says-so.md).
  it('a new gesture tears down an arm whose paired event never arrived', () => {
    installPairedSwallow();
    // No touchend, no cancel: the gesture ended on a detached node. The next
    // tap opens with its own pointerdown, which must kill the stale arm.
    dispatch('pointerdown');
    const touch = dispatch('touchend');
    expect(touch.stopPropagation).not.toHaveBeenCalled();
    expect(touch.preventDefault).not.toHaveBeenCalled();
  });

  it('the arming pointerdown itself does not tear the arm down', () => {
    // The document stub dispatches to the LIVE listener list, so the arming
    // event reaches the teardown listener it just installed. A real browser
    // iterates a copy and never does. Identity holds in both.
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

// ──────────────────────────────────────────────────────────────────────────
// The press outcome, read back. `installPairedSwallow` above writes the
// 'swallowed' half; `touchActivated` writes 'served'. Both stop an observer
// downstream of a stopPropagation from calling a live press dead.
// ──────────────────────────────────────────────────────────────────────────
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
    // How the probe reads it: `takePressOutcome(now - armedAt)`. A second
    // composer touch inside the first one's grace window supersedes it without
    // consuming its claim. Reading that stale claim would call the second
    // press served and suppress the report it was owed.
    notePressOutcome('served', 100);   // the first press was taken at t=100
    const armedAt = 200;               // the second press began at t=200
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
