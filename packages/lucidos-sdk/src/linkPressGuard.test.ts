// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { installLinkPressGuard, LINK_CLICK_AFTER_TOUCH_MS } from './linkPressGuard';
import { TAP_MOVE_THRESHOLD_PX } from './tapPress';
import { LONG_PRESS_DELAY_MS } from './widgetFrame';

const uninstalls: Array<() => void> = [];
afterEach(() => {
  while (uninstalls.length) uninstalls.pop()!();
  document.body.innerHTML = '';
});

/** A page with a link and a button, and a clock the guard reads. */
function setup() {
  const doc: Document = document;
  let now = 1_000;
  const swallowed = vi.fn();
  uninstalls.push(installLinkPressGuard(doc, { now: () => now, onSwallowed: swallowed }));
  document.body.innerHTML = '<a id="link" href="/x"><span id="inner">go</span></a><button id="plain">b</button>';
  const link = document.getElementById('inner') as HTMLElement;
  const plain = document.getElementById('plain') as HTMLElement;

  const contact = (x: number, y: number) => ({ identifier: 1, screenX: x, screenY: y });
  const touch = (type: string, x: number, y: number, left = 1) => {
    const c = contact(x, y);
    doc.dispatchEvent(Object.assign(new Event(type, { cancelable: true }), {
      changedTouches: [c],
      touches: Array.from({ length: left }, () => c),
    }));
  };
  const click = (target: HTMLElement = link) => {
    // The page's own handler: reached unless the guard stopped the click. It
    // cancels the click last, so jsdom does not try to follow the link.
    let reachedPrevented: boolean | null = null;
    const page = (e: Event) => { reachedPrevented = e.defaultPrevented; e.preventDefault(); };
    window.addEventListener('click', page);
    const e = new MouseEvent('click', { bubbles: true, cancelable: true });
    target.dispatchEvent(e);
    window.removeEventListener('click', page);
    return { prevented: reachedPrevented ?? true, reached: reachedPrevented !== null };
  };
  return {
    swallowed,
    tick: (ms: number) => { now += ms; },
    touch,
    click,
    plain,
    /** A finger down, dragged by `dx`, then lifted. */
    gesture(dx: number, { heldMs = 50, fingers = 1 }: { heldMs?: number; fingers?: number } = {}) {
      touch('touchstart', 100, 100, fingers);
      now += heldMs / 2;
      touch('touchmove', 100 + dx, 100, fingers);
      now += heldMs / 2;
      touch('touchend', 100 + dx, 100, 0);
    },
  };
}

describe('installLinkPressGuard', () => {
  it('lets a tap open its link', () => {
    const t = setup();
    t.gesture(0);
    t.tick(30);
    expect(t.click()).toEqual({ prevented: false, reached: true });
    expect(t.swallowed).not.toHaveBeenCalled();
  });

  it('swallows the click that follows a drag just past the tap threshold', () => {
    const t = setup();
    t.gesture(TAP_MOVE_THRESHOLD_PX + 1);
    t.tick(30);
    expect(t.click()).toEqual({ prevented: true, reached: false });
    expect(t.swallowed).toHaveBeenCalledOnce();
  });

  it('lets a drag of exactly the threshold through, as a tap', () => {
    const t = setup();
    t.gesture(TAP_MOVE_THRESHOLD_PX);
    expect(t.click().prevented).toBe(false);
  });

  it('swallows the click after a two-finger press', () => {
    const t = setup();
    t.gesture(0, { fingers: 2 });
    expect(t.click().prevented).toBe(true);
  });

  it('swallows the click after a press held past the long-press delay', () => {
    const t = setup();
    t.gesture(0, { heldMs: LONG_PRESS_DELAY_MS + 1 });
    expect(t.click().prevented).toBe(true);
  });

  it('counts travel the finger made and then undid', () => {
    const t = setup();
    t.touch('touchstart', 100, 100);
    t.touch('touchmove', 140, 100);
    t.touch('touchmove', 100, 100);
    t.touch('touchend', 100, 100, 0);
    expect(t.click().prevented).toBe(true);
  });

  it('swallows the click after a touch the system cancelled mid-swipe', () => {
    const t = setup();
    t.touch('touchstart', 100, 100);
    t.touch('touchmove', 160, 100);
    t.touch('touchcancel', 160, 100, 0);
    expect(t.click().prevented).toBe(true);
  });

  it('rules only on a link, never on another element', () => {
    const t = setup();
    t.gesture(60);
    expect(t.click(t.plain)).toEqual({ prevented: false, reached: true });
  });

  it('consumes the press when it rules, so a later Enter on a link opens it', () => {
    const t = setup();
    t.gesture(60);
    expect(t.click().prevented).toBe(true);
    expect(t.click().prevented).toBe(false);
  });

  it('never vetoes a click that arrives long after its touch', () => {
    const t = setup();
    t.gesture(60);
    t.tick(LINK_CLICK_AFTER_TOUCH_MS + 1);
    expect(t.click().prevented).toBe(false);
  });

  it('lets a mouse or keyboard click through when no touch came before it', () => {
    const t = setup();
    expect(t.click().prevented).toBe(false);
  });

  it('lets a click dispatched while a tap is still lifting through', () => {
    const t = setup();
    t.touch('touchstart', 100, 100);
    expect(t.click().prevented).toBe(false);
  });

  it('forgets a swipe when the next press starts', () => {
    const t = setup();
    t.gesture(60);
    t.gesture(0);
    expect(t.click().prevented).toBe(false);
  });

  it('keeps ruling touch by touch', () => {
    const t = setup();
    t.gesture(0);
    expect(t.click().prevented).toBe(false);
    t.gesture(60);
    expect(t.click().prevented).toBe(true);
  });

  it('reports what it swallowed', () => {
    const t = setup();
    t.gesture(60, { heldMs: 100 });
    t.click();
    expect(t.swallowed).toHaveBeenCalledWith({ href: '/x', movedPx: 60, fingers: 1, heldMs: 100 });
  });

  it('stops listening when uninstalled', () => {
    const t = setup();
    uninstalls.pop()!();
    t.gesture(60);
    expect(t.click().prevented).toBe(false);
  });
});
