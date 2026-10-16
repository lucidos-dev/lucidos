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
  LONG_PRESS_DELAY_MS,
  PRESS_MOVE_TOLERANCE_PX,
  WIDGET_CHANNEL,
  WIDGET_HOLD_MESSAGE_TYPE,
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_PLACE_ATTRIBUTE,
  WIDGET_SIZE_ATTRIBUTE,
  WIDGET_TAP_MESSAGE_TYPE,
  _resetWidgetFrameForTesting,
  contentHeight,
  installWidgetFrame,
  widget,
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
  const pointer = (type: string, pointerType: string, y = 0) => {
    const e = new Event(type, { bubbles: true });
    Object.assign(e, { isPrimary: true, pointerType, pointerId: 1, screenX: 0, screenY: y });
    return e;
  };
  /** A press that lifts where it went down: a tap. */
  const press = (pointerType: string) => {
    document.body.dispatchEvent(pointer('pointerdown', pointerType));
    window.dispatchEvent(pointer('pointerup', pointerType));
  };
  const taps = () => posted.filter((m) => (m as { type?: string }).type === WIDGET_TAP_MESSAGE_TYPE);

  beforeEach(() => {
    posted.length = 0;
    pushHandlers.clear();
    _resetWidgetFrameForTesting();
    Object.defineProperty(window, 'parent', { configurable: true, value: parent });
    vi.stubGlobal('requestAnimationFrame', (run: () => void) => { run(); return 0; });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    Object.defineProperty(document.body, 'scrollHeight', { configurable: true, value: 200 });
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
    expect(posted).toEqual([{ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 200 }]);
  });

  it('every mark reports, so the host hears the size in a new place', () => {
    installWidgetFrame();
    mark({ place: 'shelf' });
    mark({ place: 'shelf' });
    expect(posted).toEqual([
      { type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 200 },
      { type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 200 },
    ]);
  });

  it('a frame outside a Home board tile posts no tap', () => {
    installWidgetFrame();
    mark({ place: 'inline' });
    press('touch');
    expect(taps()).toEqual([]);
  });

  it('a finger that scrolls the Home board posts no tap', () => {
    installWidgetFrame();
    mark({ place: 'home', size: 'small' });
    // The browser takes the touch for the scroll and cancels it.
    document.body.dispatchEvent(pointer('pointerdown', 'touch'));
    window.dispatchEvent(pointer('pointercancel', 'touch'));
    // A lift after travel past the tolerance is a drag, not a tap.
    document.body.dispatchEvent(pointer('pointerdown', 'touch'));
    window.dispatchEvent(pointer('pointermove', 'touch', PRESS_MOVE_TOLERANCE_PX + 1));
    window.dispatchEvent(pointer('pointerup', 'touch', PRESS_MOVE_TOLERANCE_PX + 1));
    expect(taps()).toEqual([]);
  });

  it('a Home board tile posts a tap for a still lift, jitter included', () => {
    installWidgetFrame();
    mark({ place: 'home', size: 'small' });
    document.body.dispatchEvent(pointer('pointerdown', 'touch'));
    window.dispatchEvent(pointer('pointermove', 'touch', PRESS_MOVE_TOLERANCE_PX));
    window.dispatchEvent(pointer('pointerup', 'touch', PRESS_MOVE_TOLERANCE_PX));
    expect(taps()).toEqual([{ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: 'touch' }]);
  });

  it('a Home board tile stamps its size and place, and lucidos.widget reads them', () => {
    const root = document.documentElement;
    installWidgetFrame();
    expect(widget.size()).toBeNull();
    expect(widget.place()).toBeNull();
    mark({ place: 'home', size: 'large' });
    expect(root.getAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe('large');
    expect(root.getAttribute(WIDGET_PLACE_ATTRIBUTE)).toBe('home');
    expect(widget.size()).toBe('large');
    expect(widget.place()).toBe('home');

    mark({ place: 'inline' });
    expect(root.hasAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe(false);
    expect(widget.size()).toBeNull();
    expect(widget.place()).toBe('inline');
  });

  it('keeps a Home board tile from scrolling inside, whatever stylesheet the widget links', () => {
    installWidgetFrame();
    mark({ place: 'home', size: 'small' });
    expect(document.documentElement.style.getPropertyValue('overflow')).toBe('hidden');
    expect(document.body.style.getPropertyValue('overflow')).toBe('hidden');
    mark({ place: 'inline' });
    expect(document.documentElement.style.getPropertyValue('overflow')).toBe('');
  });

  it('an unknown size or place stamps nothing', () => {
    installWidgetFrame();
    mark({ place: 'desk', size: 'huge' });
    expect(document.documentElement.hasAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe(false);
    expect(document.documentElement.hasAttribute(WIDGET_PLACE_ATTRIBUTE)).toBe(false);
  });

  it('onSize hears each new size once, and stops when unsubscribed', () => {
    installWidgetFrame();
    const heard: unknown[] = [];
    const off = widget.onSize((s) => heard.push(s));
    mark({ place: 'home', size: 'small' });
    mark({ place: 'home', size: 'small' });
    mark({ place: 'home', size: 'medium' });
    off();
    mark({ place: 'home', size: 'large' });
    expect(heard).toEqual(['small', 'medium']);
  });

  describe('a finger held on a Home board tile', () => {
    const finger = (type: string, x = 0) => {
      const e = new Event(type, { bubbles: true });
      Object.assign(e, { isPrimary: true, pointerType: 'touch', pointerId: 1, screenX: x, screenY: 0 });
      return e;
    };
    const holds = () => posted.filter((m) => (m as { type?: string }).type === WIDGET_HOLD_MESSAGE_TYPE);

    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('posts a hold once the press outlasts a long press, and no tap on its lift', () => {
      installWidgetFrame();
      mark({ place: 'home', size: 'small' });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      window.dispatchEvent(finger('pointerup'));
      expect(holds()).toHaveLength(1);
      expect(taps()).toEqual([]);
    });

    it('posts none when the finger moves first, lifts first, or the frame is no tile', () => {
      installWidgetFrame();
      mark({ place: 'home', size: 'small' });
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointermove', PRESS_MOVE_TOLERANCE_PX + 1));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointerup'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      mark({ place: 'inline' });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(holds()).toHaveLength(0);
    });
  });

  it('onVisibility hears a hidden tile and its return', () => {
    installWidgetFrame();
    const heard: boolean[] = [];
    widget.onVisibility((v) => heard.push(v));
    mark({ place: 'home', size: 'small' });
    expect(widget.visible()).toBe(true);
    mark({ place: 'home', size: 'small', visible: false });
    expect(widget.visible()).toBe(false);
    mark({ place: 'home', size: 'small', visible: true });
    expect(heard).toEqual([false, true]);
  });
});
