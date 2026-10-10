// @vitest-environment jsdom
/**
 * Widget windows (ADR 0419): several float at once, a tap elsewhere closes
 * none of them, a drag on the bar moves one, and only Close shuts it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../WidgetFrame', () => ({
  WidgetFrame: ({ appId }: { appId: string }) => <iframe data-frame={appId} />,
}));

import type { ThreadWidget } from '../../../api/client/widgets';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { openWidgetWindow, widgetWindows } from '../../../store/widgetWindows';
import { widgetInstanceKey } from '../../../utils/widgetParams';
import { WidgetWindows, WIDGET_WINDOW_FADE_MS } from '../WidgetWindows';

let host: HTMLDivElement;

function widget(appId: string, pinned = true): ThreadWidget {
  return { app_id: appId, name: appId, reusable: false, reveal: 'on-load', pinned, shown_event_id: `ev-${appId}` };
}

/** A widget with no params is the instance its app id names. */
const k = (appId: string) => widgetInstanceKey(appId, undefined);

function setHomeWidgets(data: ThreadWidget[]): void {
  threadWidgets.value = new Map([['home', { status: 'loaded', data }]]);
}

function pointer(type: string, x = 0, y = 0): PointerEvent {
  // jsdom has no PointerEvent constructor; the handlers read `button` and
  // `clientX/Y`.
  return new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y }) as unknown as PointerEvent;
}

async function flush(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

/** `act` runs the effects a render queued, which fake timers would hold. */
const mount = () => act(() => { render(<WidgetWindows />, host); });

const shown = () => [...document.querySelectorAll<HTMLElement>('[data-widget-window]')]
  .map((el) => el.dataset.widgetWindow?.split('?')[0]);
const win = (appId: string) => document.querySelector<HTMLElement>(`[data-widget-window="${CSS.escape(k(appId))}"]`)!;

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  widgetWindows.value = [];
  HTMLElement.prototype.setPointerCapture = vi.fn();
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true } })]]);
  setHomeWidgets([widget('fares'), widget('weather')]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('widget windows', () => {
  it('draws one per open Home widget, and none for another workspace\'s', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 10 }, 24);
    openWidgetWindow('elsewhere-home', k('fares'), { x: 10, y: 10 }, 24);
    mount();
    await flush();
    expect(shown()).toEqual(['fares', 'weather']);
  });

  it('keeps every window open through a tap elsewhere in the app', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 10 }, 24);
    mount();
    document.body.dispatchEvent(pointer('pointerdown'));
    document.body.click();
    await flush();
    expect(shown()).toEqual(['fares', 'weather']);
  });

  it('fades out on Close, then goes', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    mount();
    act(() => win('fares').querySelector<HTMLButtonElement>('button[aria-label="Close fares"]')!.click());
    expect(win('fares').classList.contains('is-closing')).toBe(true);
    act(() => { vi.advanceTimersByTime(WIDGET_WINDOW_FADE_MS + 100); });
    expect(shown()).toEqual([]);
    expect(widgetWindows.value).toEqual([]);
  });

  it('picking a widget again keeps the window its Close is fading out', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    mount();
    act(() => win('fares').querySelector<HTMLButtonElement>('button[aria-label="Close fares"]')!.click());
    act(() => openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24));
    act(() => { vi.advanceTimersByTime(WIDGET_WINDOW_FADE_MS + 100); });
    expect(shown()).toEqual(['fares']);
    expect(win('fares').classList.contains('is-closing')).toBe(false);
  });

  it('draws one window per instance of a widget pinned twice', async () => {
    const week = { ...widget('habits'), params: { range: 'week' }, label: 'This week' };
    const month = { ...widget('habits'), params: { range: 'month' }, label: 'This month' };
    setHomeWidgets([week, month]);
    openWidgetWindow('home', widgetInstanceKey('habits', { range: 'month' }), { x: 10, y: 10 }, 24);
    mount();
    expect([...document.querySelectorAll<HTMLElement>('[data-widget-window]')].map((el) => el.getAttribute('aria-label')))
      .toEqual(['This month widget']);
    act(() => setHomeWidgets([week, { ...month, pinned: false }]));
    expect(shown()).toEqual([]);
  });

  it('closes when its widget is unpinned from Home', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 10 }, 24);
    mount();
    act(() => setHomeWidgets([widget('fares', false), widget('weather')]));
    expect(shown()).toEqual(['weather']);
    expect(widgetWindows.value.map((w) => w.instanceKey)).toEqual([k('weather')]);
  });

  it('raises a pressed window without moving any frame in the DOM', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 10 }, 24);
    mount();
    const frame = win('fares').querySelector('iframe');
    win('fares').dispatchEvent(pointer('pointerdown'));
    await flush();
    expect(shown()).toEqual(['fares', 'weather']);
    expect(win('fares').querySelector('iframe')).toBe(frame);
    expect(win('fares').style.zIndex).toBe('calc(var(--z-widget-window) + 1)');
    expect(win('weather').style.zIndex).toBe('calc(var(--z-widget-window) + 0)');
  });

  it('raises a window whose frame takes focus, as a press inside it does', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 10 }, 24);
    mount();
    act(() => { win('fares').querySelector('iframe')!.dispatchEvent(new FocusEvent('focusin', { bubbles: true })); });
    expect(win('fares').style.zIndex).toBe('calc(var(--z-widget-window) + 1)');
  });

  it('moves where its bar is dragged', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const bar = win('fares').querySelector<HTMLElement>('.widget-bar-name')!;
    bar.dispatchEvent(pointer('pointerdown', 100, 100));
    bar.dispatchEvent(pointer('pointermove', 150, 180));
    bar.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(widgetWindows.value.map((w) => [w.x, w.y])).toEqual([[60, 100]]);
    expect(win('fares').style.left).toBe('60px');
  });

  it('does not drag from a press on one of the bar\'s buttons', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const close = win('fares').querySelector<HTMLElement>('button[aria-label="Close fares"]')!;
    close.dispatchEvent(pointer('pointerdown', 100, 100));
    close.dispatchEvent(pointer('pointermove', 150, 180));
    close.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(widgetWindows.value.map((w) => [w.x, w.y])).toEqual([[10, 20]]);
  });
});
