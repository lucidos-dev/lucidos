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
  WIDGET_HEIGHT_MESSAGE_TYPE,
  _resetWidgetHeightForTesting,
  contentHeight,
  installWidgetHeight,
} from './widgetHeight';

describe('contentHeight', () => {
  it('measures the body and its margins, not the viewport', () => {
    Object.defineProperty(document.body, 'scrollHeight', { configurable: true, value: 120.4 });
    document.body.style.margin = '8px';
    expect(contentHeight(document)).toBe(137);
  });
});

describe('installWidgetHeight', () => {
  const posted: unknown[] = [];
  const parent = { postMessage: (message: unknown) => posted.push(message) };

  beforeEach(() => {
    posted.length = 0;
    pushHandlers.clear();
    _resetWidgetHeightForTesting();
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
    installWidgetHeight();
    expect(posted).toEqual([]);
    pushHandlers.get(WIDGET_CHANNEL)?.({});
    expect(posted).toEqual([{ type: WIDGET_HEIGHT_MESSAGE_TYPE, height: 200 }]);
  });

  it('a second mark starts no second reporter', () => {
    installWidgetHeight();
    pushHandlers.get(WIDGET_CHANNEL)?.({});
    pushHandlers.get(WIDGET_CHANNEL)?.({});
    expect(posted).toHaveLength(1);
  });
});
