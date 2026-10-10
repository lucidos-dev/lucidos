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
