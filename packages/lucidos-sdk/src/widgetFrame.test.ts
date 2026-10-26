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
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_MENU_MESSAGE_TYPE,
  WIDGET_PLACE_ATTRIBUTE,
  WIDGET_SIZE_ATTRIBUTE,
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
  const menus = () => posted.filter((m) => (m as { type?: string }).type === WIDGET_MENU_MESSAGE_TYPE);

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

  it('a dashboard tile stamps its size and place, and lucidos.widget reads them', () => {
    const root = document.documentElement;
    installWidgetFrame();
    expect(widget.size()).toBeNull();
    expect(widget.place()).toBeNull();
    mark({ place: 'dashboard', size: 'large' });
    expect(root.getAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe('large');
    expect(root.getAttribute(WIDGET_PLACE_ATTRIBUTE)).toBe('dashboard');
    expect(widget.size()).toBe('large');
    expect(widget.place()).toBe('dashboard');

    mark({ place: 'inline' });
    expect(root.hasAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe(false);
    expect(widget.size()).toBeNull();
    expect(widget.place()).toBe('inline');
  });

  it('a repeat push writes nothing to <html>, and leaves a size the widget pinned itself', async () => {
    const root = document.documentElement;
    root.setAttribute(WIDGET_SIZE_ATTRIBUTE, 'medium');
    const records: MutationRecord[] = [];
    const observer = new MutationObserver((batch) => records.push(...batch));
    observer.observe(root, { attributes: true });
    installWidgetFrame();
    mark({ place: 'inline' });
    await Promise.resolve();
    expect(records.map((r) => r.attributeName)).toEqual([WIDGET_PLACE_ATTRIBUTE]);

    records.length = 0;
    mark({ place: 'inline' });
    mark({ place: 'inline' });
    await Promise.resolve();
    observer.disconnect();
    expect(records).toEqual([]);
    expect(root.getAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe('medium');
  });

  it('a real change from the host still stamps, a size going back to none included', () => {
    const root = document.documentElement;
    installWidgetFrame();
    mark({ place: 'dashboard', size: 'small' });
    expect(root.getAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe('small');
    mark({ place: 'dashboard', size: 'medium' });
    expect(root.getAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe('medium');
    mark({ place: 'dashboard' });
    expect(root.hasAttribute(WIDGET_SIZE_ATTRIBUTE)).toBe(false);
    expect(root.getAttribute(WIDGET_PLACE_ATTRIBUTE)).toBe('dashboard');
  });

  it('keeps a dashboard tile from scrolling inside, whatever stylesheet the widget links', () => {
    installWidgetFrame();
    mark({ place: 'dashboard', size: 'small' });
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
    mark({ place: 'dashboard', size: 'small' });
    mark({ place: 'dashboard', size: 'small' });
    mark({ place: 'dashboard', size: 'medium' });
    off();
    mark({ place: 'dashboard', size: 'large' });
    expect(heard).toEqual(['small', 'medium']);
  });

  describe('a finger held on a dashboard tile', () => {
    const finger = (type: string, x = 0) => {
      const e = new Event(type, { bubbles: true });
      Object.assign(e, { isPrimary: true, button: 0, pointerType: 'touch', pointerId: 1, screenX: x, screenY: 0 });
      return e;
    };
    const longPressMenu = () => {
      const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
      document.body.dispatchEvent(e);
      return e;
    };

    beforeEach(() => vi.useFakeTimers());
    afterEach(() => {
      vi.useRealTimers();
      document.body.innerHTML = '';
    });

    it('posts a held menu request with no point once a press outlasts a long press, finger still down', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 1);
      expect(menus()).toHaveLength(0);
      vi.advanceTimersByTime(1);
      // The lift's click is still to come, and the host's menu must spend it.
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, held: true }]);
      window.dispatchEvent(finger('pointerup'));
      expect(menus()).toHaveLength(1);
    });

    it('a new press ends one whose lift was lost before its delay', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 100);
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointerup'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(menus()).toHaveLength(0);
    });

    it("swallows the platform's long-press menu that follows the hold, so one press opens one menu", () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(longPressMenu().defaultPrevented).toBe(true);
      expect(menus()).toHaveLength(1);
    });

    it("a press turns off iOS's own long-press answer at once, until the next press", () => {
      const root = document.documentElement;
      document.body.innerHTML = '<p>Twelve minutes left</p>';
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      // iOS shows its own selection or link menu while the finger rests, so the
      // press itself must turn them off. jsdom drops the callout property.
      for (const property of ['user-select', '-webkit-user-select']) {
        expect(root.style.getPropertyValue(property)).toBe('none');
      }
      window.getSelection()?.selectAllChildren(document.body);
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(window.getSelection()?.rangeCount).toBe(0);
      window.dispatchEvent(finger('pointerup'));
      expect(root.style.getPropertyValue('user-select')).toBe('none');
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointerup'));
      mark({ place: 'inline', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      for (const property of ['user-select', '-webkit-user-select']) {
        expect(root.style.getPropertyValue(property)).toBe('');
      }
    });

    it("the next press gives back the widget's own selection style", () => {
      const root = document.documentElement;
      root.style.setProperty('user-select', 'text');
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      expect(root.style.getPropertyValue('user-select')).toBe('none');
      mark({ place: 'inline', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      expect(root.style.getPropertyValue('user-select')).toBe('text');
      expect(root.style.getPropertyPriority('user-select')).toBe('');
      root.style.removeProperty('user-select');
    });

    it('a hold that opens the menu ticks, and a quick tap does not', () => {
      const vibrate = vi.fn();
      Object.defineProperty(navigator, 'vibrate', { value: vibrate, configurable: true });
      try {
        installWidgetFrame();
        mark({ place: 'dashboard', size: 'small', menu: true });
        document.body.dispatchEvent(finger('pointerdown'));
        window.dispatchEvent(finger('pointerup'));
        expect(vibrate).not.toHaveBeenCalled();
        document.body.dispatchEvent(finger('pointerdown'));
        vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
        expect(vibrate).toHaveBeenCalledTimes(1);
      } finally {
        delete (navigator as { vibrate?: unknown }).vibrate;
      }
    });

    it("a long-press menu that comes first cancels the hold", () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      longPressMenu();
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      // The finger is still down, so its lift may still bring a click.
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, held: true }]);
    });

    it('a long-press menu that reports the finger as a button still sends no point', () => {
      installWidgetFrame();
      mark({ place: 'shelf', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      document.body.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 9, clientY: 9, buttons: 1 }));
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, held: true }]);
    });

    it('a starved frame reads a long hold from the event times, and the lift opens the menu', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      const at = (type: string, timeStamp: number) => Object.defineProperty(finger(type), 'timeStamp', { value: timeStamp });
      // Both events reach a frame that was busy for the whole press, so no timer ran.
      document.body.dispatchEvent(at('pointerdown', 1000));
      window.dispatchEvent(at('pointerup', 1000 + LONG_PRESS_DELAY_MS));
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, held: true }]);
    });

    it('a quick lift in a starved frame opens no menu', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      const at = (type: string, timeStamp: number) => Object.defineProperty(finger(type), 'timeStamp', { value: timeStamp });
      document.body.dispatchEvent(at('pointerdown', 1000));
      window.dispatchEvent(at('pointerup', 1000 + LONG_PRESS_DELAY_MS - 1));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(menus()).toHaveLength(0);
    });

    it('posts none when the finger moves first, lifts first, or the frame is no tile', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointermove', PRESS_MOVE_TOLERANCE_PX + 1));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      document.body.dispatchEvent(finger('pointerdown'));
      window.dispatchEvent(finger('pointerup'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      mark({ place: 'inline', menu: true });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(menus()).toHaveLength(0);
    });

    it('posts none on a tile the host did not mark for a menu', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', size: 'small' });
      document.body.dispatchEvent(finger('pointerdown'));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
      expect(menus()).toHaveLength(0);
    });
  });

  describe('a right-click in a widget frame', () => {
    const at = { x: 40, y: 24 };
    /** A secondary-button press, then the menu event, both on `target`. */
    const rightClick = (target: Element, init: MouseEventInit = {}) => {
      target.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 2, clientX: at.x, clientY: at.y, ...init }));
      const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: at.x, clientY: at.y, ...init });
      target.dispatchEvent(e);
      return e;
    };
    const place = (html: string) => {
      document.body.innerHTML = html;
      return document.body.querySelector('[data-pick]') ?? document.body.firstElementChild!;
    };
    afterEach(() => {
      document.body.innerHTML = '';
      vi.restoreAllMocks();
    });

    it('opens the widget menu at the pointer in a frame the host marked', () => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      const e = rightClick(place('<div data-pick>chart</div>'));
      expect(e.defaultPrevented).toBe(true);
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, at }]);
    });

    it('sends no point for a menu the keyboard raised', () => {
      installWidgetFrame();
      mark({ place: 'shelf', menu: true });
      const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
      place('<div data-pick>chart</div>').dispatchEvent(e);
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE }]);
    });

    it('opens at the pointer for a right-click whose press the frame never saw', () => {
      installWidgetFrame();
      mark({ place: 'dashboard', menu: true });
      const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: at.x, clientY: at.y, buttons: 2 });
      place('<div data-pick>chart</div>').dispatchEvent(e);
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE, at }]);
    });

    it('leaves every right-click alone in a frame the host did not mark', () => {
      installWidgetFrame();
      mark({ place: 'option' });
      const e = rightClick(place('<div data-pick>chart</div>'));
      expect(e.defaultPrevented).toBe(false);
      expect(menus()).toEqual([]);
    });

    it('Option+right-click keeps the native menu', () => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      const e = rightClick(place('<div data-pick>chart</div>'), { altKey: true });
      expect(e.defaultPrevented).toBe(false);
      expect(menus()).toEqual([]);
    });

    it("a widget that claims the right-click itself keeps it", () => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      const target = place('<div data-pick>canvas</div>');
      target.addEventListener('contextmenu', (e) => e.preventDefault());
      rightClick(target);
      document.addEventListener('contextmenu', (e) => e.preventDefault(), { once: true });
      rightClick(target);
      expect(menus()).toEqual([]);
    });

    it('a right-click the widget stopped leaves no point for the next keyboard menu', () => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      const target = place('<div data-pick>canvas</div>');
      const claim = (e: Event) => { e.preventDefault(); e.stopPropagation(); };
      target.addEventListener('contextmenu', claim);
      rightClick(target);
      target.removeEventListener('contextmenu', claim);
      target.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: at.x, clientY: at.y }));
      expect(menus()).toEqual([{ type: WIDGET_MENU_MESSAGE_TYPE }]);
    });

    it.each([
      ['a link', '<a href="https://example.com"><span data-pick>docs</span></a>'],
      ['a text field', '<input data-pick>'],
    ])('keeps the native menu on %s', (_label, html) => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      const e = rightClick(place(html));
      expect(e.defaultPrevented).toBe(false);
      expect(menus()).toEqual([]);
    });

    it('keeps the native menu over a selection the user made before pressing', () => {
      installWidgetFrame();
      mark({ place: 'inline', menu: true });
      vi.spyOn(window, 'getSelection').mockReturnValue({
        isCollapsed: false,
        rangeCount: 1,
        getRangeAt: () => ({ getClientRects: () => [{ left: 0, right: 100, top: 0, bottom: 100 }] }),
        removeAllRanges: () => {},
      } as unknown as Selection);
      const e = rightClick(place('<div data-pick>chart</div>'));
      expect(e.defaultPrevented).toBe(false);
      expect(menus()).toEqual([]);
    });
  });

  it('onVisibility hears a hidden tile and its return', () => {
    installWidgetFrame();
    const heard: boolean[] = [];
    widget.onVisibility((v) => heard.push(v));
    mark({ place: 'dashboard', size: 'small' });
    expect(widget.visible()).toBe(true);
    mark({ place: 'dashboard', size: 'small', visible: false });
    expect(widget.visible()).toBe(false);
    mark({ place: 'dashboard', size: 'small', visible: true });
    expect(heard).toEqual([false, true]);
  });
});
