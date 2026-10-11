// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const pushHandlers = new Map<string, (data: unknown) => void>();
vi.mock('./_bridge', () => ({
  onHostPush: (channel: string, handler: (data: unknown) => void) => {
    pushHandlers.set(channel, handler);
    return () => pushHandlers.delete(channel);
  },
}));

import {
  WIDGET_CHANNEL,
  WIDGET_DRAG_MESSAGE_TYPE,
  WIDGET_DRAG_THRESHOLD_PX,
  WIDGET_MODE_ATTRIBUTE,
  WIDGET_SIZE_MESSAGE_TYPE,
  WIDGET_TAP_MESSAGE_TYPE,
  _resetWidgetFrameForTesting,
  contentHeight,
  installWidgetFrame,
} from './widgetFrame';

describe('contentHeight', () => {
  it('measures the body and its margins, not the viewport', () => {
    Object.defineProperty(document.body, 'scrollHeight', { configurable: true, value: 120.4 });
    document.body.style.margin = '8px';
    expect(contentHeight(document)).toBe(137);
  });
});

describe('installWidgetFrame', () => {
  const posted: unknown[] = [];
  const parent = { postMessage: (message: unknown) => posted.push(message) };
  const mark = (data: unknown) => pushHandlers.get(WIDGET_CHANNEL)?.(data);
  const press = (pointerType: string) => {
    const e = new Event('pointerdown', { bubbles: true });
    Object.assign(e, { isPrimary: true, pointerType });
    document.body.dispatchEvent(e);
  };

  beforeEach(() => {
    posted.length = 0;
    pushHandlers.clear();
    _resetWidgetFrameForTesting();
    Object.defineProperty(window, 'parent', { configurable: true, value: parent });
    vi.stubGlobal('requestAnimationFrame', (run: () => void) => { run(); return 0; });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    Object.defineProperty(document.body, 'scrollHeight', { configurable: true, value: 200 });
    document.body.getBoundingClientRect = () => ({ width: 180.2 }) as DOMRect;
    document.body.style.margin = '0';
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    Object.defineProperty(window, 'parent', { configurable: true, value: window });
  });

  it('posts nothing until the host marks the frame a widget', () => {
    installWidgetFrame();
    expect(posted).toEqual([]);
    mark({});
    expect(posted).toEqual([{ type: WIDGET_SIZE_MESSAGE_TYPE, height: 200 }]);
  });

  it('every mark reports, so the host hears the size in a new mode', () => {
    installWidgetFrame();
    mark({ mode: 'docked' });
    mark({ mode: 'window' });
    expect(posted).toEqual([
      { type: WIDGET_SIZE_MESSAGE_TYPE, height: 200 },
      { type: WIDGET_SIZE_MESSAGE_TYPE, height: 200 },
    ]);
  });

  it('a frame outside a widget window has no mode, reports no width and posts no tap', () => {
    installWidgetFrame();
    mark({});
    press('touch');
    expect(document.documentElement.hasAttribute(WIDGET_MODE_ATTRIBUTE)).toBe(false);
    expect(posted).toEqual([{ type: WIDGET_SIZE_MESSAGE_TYPE, height: 200 }]);
  });

  it('minimal mode stamps the mode and reports the width', () => {
    installWidgetFrame();
    mark({ mode: 'minimal' });
    expect(document.documentElement.getAttribute(WIDGET_MODE_ATTRIBUTE)).toBe('minimal');
    expect(posted).toEqual([{ type: WIDGET_SIZE_MESSAGE_TYPE, height: 200, width: 181 }]);
  });

  it('a mode change restamps and reports again, without the width outside minimal', () => {
    installWidgetFrame();
    mark({ mode: 'minimal' });
    mark({ mode: 'window' });
    expect(document.documentElement.getAttribute(WIDGET_MODE_ATTRIBUTE)).toBe('window');
    expect(posted[posted.length - 1]).toEqual({ type: WIDGET_SIZE_MESSAGE_TYPE, height: 200 });
  });

  it('a press in a window frame posts a tap with its pointer type', () => {
    installWidgetFrame();
    mark({ mode: 'docked' });
    press('touch');
    expect(posted[posted.length - 1]).toEqual({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: 'touch' });
  });
});

describe('dragging a movable widget window from inside the widget', () => {
  const posted: { type?: string }[] = [];
  const parent = { postMessage: (message: { type?: string }) => posted.push(message) };
  const mark = (data: unknown) => pushHandlers.get(WIDGET_CHANNEL)?.(data);
  const drags = () => posted.filter((m) => m.type === WIDGET_DRAG_MESSAGE_TYPE);

  /** A pointer event the way jsdom can carry one: a mouse event plus the
   *  pointer fields the handlers read. */
  function pointer(type: string, x: number, y: number, extra: Record<string, unknown> = {}): Event {
    // Only a move carries the held button; a release has none left.
    const buttons = type === 'pointermove' ? 1 : 0;
    const e = new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons, screenX: x, screenY: y });
    for (const [key, value] of Object.entries({ isPrimary: true, pointerType: 'mouse', pointerId: 1, offsetX: 1, offsetY: 1, ...extra })) {
      Object.defineProperty(e, key, { value });
    }
    return e;
  }

  /** Presses `el`, travels `far` pixels and lets go. */
  function drag(el: Element, far = 40, extra: Record<string, unknown> = {}): void {
    el.dispatchEvent(pointer('pointerdown', 100, 100, extra));
    window.dispatchEvent(pointer('pointermove', 100 + far, 100 + far, extra));
    window.dispatchEvent(pointer('pointerup', 100 + far, 100 + far, extra));
  }

  let el: HTMLElement;
  beforeEach(() => {
    posted.length = 0;
    pushHandlers.clear();
    _resetWidgetFrameForTesting();
    Object.defineProperty(window, 'parent', { configurable: true, value: parent });
    vi.stubGlobal('requestAnimationFrame', () => 0);
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    document.body.innerHTML = '<p>Total</p>';
    el = document.querySelector('p')!;
    installWidgetFrame();
    mark({ mode: 'minimal', draggable: true });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    Object.defineProperty(window, 'parent', { configurable: true, value: window });
    document.body.innerHTML = '';
  });

  it('posts the drag from where it was pressed, each step, and its end', () => {
    drag(el);
    expect(drags()).toEqual([
      { type: WIDGET_DRAG_MESSAGE_TYPE, phase: 'start', screenX: 100, screenY: 100 },
      { type: WIDGET_DRAG_MESSAGE_TYPE, phase: 'move', screenX: 140, screenY: 140 },
      { type: WIDGET_DRAG_MESSAGE_TYPE, phase: 'end', screenX: 140, screenY: 140 },
    ]);
  });

  it('a cancelled drag ends where the pointer last was, not at the cancel\'s coordinates', () => {
    el.dispatchEvent(pointer('pointerdown', 100, 100));
    window.dispatchEvent(pointer('pointermove', 140, 140));
    window.dispatchEvent(pointer('pointercancel', 0, 0));
    expect(drags()[drags().length - 1]).toEqual({ type: WIDGET_DRAG_MESSAGE_TYPE, phase: 'end', screenX: 140, screenY: 140 });
  });

  it('a click stays a click: no travel past the threshold posts no drag', () => {
    drag(el, (WIDGET_DRAG_THRESHOLD_PX - 1) / 2);
    expect(drags()).toEqual([]);
  });

  it('never starts on a control, inside one, or on a role the widget answers', () => {
    document.body.innerHTML = '<button>Go</button><a href="#"><span>link</span></a><div role="slider">s</div><input>'
      + '<label>Name</label><details><summary>More</summary></details><div contenteditable="true">Note</div>';
    for (const target of ['button', 'a span', '[role="slider"]', 'input', 'label', 'summary', '[contenteditable]']) {
      drag(document.querySelector(target)!);
    }
    expect(drags()).toEqual([]);
  });

  it('never starts on something drawn clickable, or on a press the widget handled', () => {
    document.body.innerHTML = '<div class="card" style="cursor: pointer">Open</div><div class="handled">Pan</div>';
    drag(document.querySelector('.card')!);
    const handled = document.querySelector('.handled')!;
    handled.addEventListener('pointerdown', (e) => e.preventDefault());
    drag(handled);
    expect(drags()).toEqual([]);
  });

  it('never starts on a scrollbar or from a finger, which scrolls', () => {
    Object.defineProperty(el, 'clientWidth', { configurable: true, value: 50 });
    drag(el, 40, { offsetX: 55 });
    drag(el, 40, { pointerType: 'touch' });
    expect(drags()).toEqual([]);
  });

  it('a press released outside the frame ends there, so a later hover never drags', () => {
    el.dispatchEvent(pointer('pointerdown', 100, 100));
    window.dispatchEvent(pointer('pointermove', 160, 160, { buttons: 0 }));
    window.dispatchEvent(pointer('pointermove', 200, 200));
    expect(drags()).toEqual([]);
  });

  it('a window that cannot move gets no drag', () => {
    mark({ mode: 'docked', draggable: false });
    drag(el);
    expect(drags()).toEqual([]);
  });
});
