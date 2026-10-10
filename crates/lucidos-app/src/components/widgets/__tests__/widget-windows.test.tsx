// @vitest-environment jsdom
/**
 * Widget windows (ADR 0419): several float at once, a tap elsewhere closes
 * none of them, a drag moves one, and only Close shuts it. Each draws in one
 * of three modes and comes back as it was left.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

type OnSize = (size: { height: number; width?: number }, fromReport: boolean) => void;
const frameProps = new Map<string, { mode?: string; onSize?: OnSize }>();
vi.mock('../WidgetFrame', () => ({
  WidgetFrame: (props: { appId: string; mode?: string; onSize?: OnSize }) => {
    frameProps.set(props.appId, props);
    return <iframe data-frame={props.appId} />;
  },
}));

import type { ThreadWidget } from '../../../api/client/widgets';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { APP_FRAME_TAP_EVENT } from '../../../store/actions/widget-frame-bridge';
import {
  closeWidgetWindow,
  openWidgetWindow,
  resizeWidgetWindow,
  setWidgetWindowMode,
  widgetWindowLayout,
  widgetWindowLayouts,
  widgetWindows,
} from '../../../store/widgetWindows';
import { viewportIsMobile } from '../../../utils/viewport';
import { widgetInstanceKey } from '../../../utils/widgetParams';
import { CONTROLS_REVEAL_MS, WidgetWindows, WIDGET_WINDOW_FADE_MS } from '../WidgetWindows';

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
const placed = (appId: string) => {
  const { x, y } = widgetWindowLayout('home', k(appId));
  return [x, y];
};
/** The widget reporting its size, as the SDK does through the frame. */
const report = (appId: string, size: { height: number; width?: number }) =>
  act(() => frameProps.get(appId)?.onSize?.(size, true));

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  widgetWindows.value = [];
  widgetWindowLayouts.value = [];
  viewportIsMobile.value = false;
  frameProps.clear();
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

  it('forgets the layout of a widget unpinned from Home', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 10 }, 24);
    act(() => closeWidgetWindow('home', k('fares')));
    mount();
    act(() => setHomeWidgets([widget('fares', false), widget('weather')]));
    expect(widgetWindowLayouts.value).toEqual([]);
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
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const bar = win('fares').querySelector<HTMLElement>('.widget-bar-name')!;
    bar.dispatchEvent(pointer('pointerdown', 100, 100));
    bar.dispatchEvent(pointer('pointermove', 150, 180));
    bar.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(placed('fares')).toEqual([60, 100]);
    expect(win('fares').style.left).toBe('60px');
  });

  it('does not drag from a press on one of the bar\'s buttons', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const close = win('fares').querySelector<HTMLElement>('button[aria-label="Close fares"]')!;
    close.dispatchEvent(pointer('pointerdown', 100, 100));
    close.dispatchEvent(pointer('pointermove', 150, 180));
    close.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(placed('fares')).toEqual([10, 20]);
  });

  it('may sit over the header: its bounds are the whole viewport', async () => {
    openWidgetWindow('home', k('fares'), { x: 0, y: 0 }, 24);
    mount();
    expect(win('fares').style.top).toBe('0px');
  });
});

describe('minimal mode', () => {
  it('is how a new window opens on a desktop: no bar, floating controls', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(win('fares').classList.contains('is-minimal')).toBe(true);
    expect(win('fares').querySelector('.widget-bar')).toBeNull();
    expect(win('fares').querySelector('.widget-window-controls')).not.toBeNull();
    expect(frameProps.get('fares')?.mode).toBe('minimal');
  });

  it('takes the width and height the widget reports', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    report('fares', { height: 90, width: 180 });
    expect(win('fares').style.getPropertyValue('--widget-content-width')).toBe('180px');
    expect(win('fares').style.getPropertyValue('--widget-content-height')).toBe('90px');
  });

  it('drags by its grip', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const grip = win('fares').querySelector<HTMLElement>('.widget-window-grip')!;
    grip.dispatchEvent(pointer('pointerdown', 100, 100));
    grip.dispatchEvent(pointer('pointermove', 150, 180));
    grip.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(placed('fares')).toEqual([60, 100]);
  });

  it('a tap inside the widget shows the controls for a while', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const frame = win('fares').querySelector('iframe')!;
    act(() => { frame.dispatchEvent(new CustomEvent(APP_FRAME_TAP_EVENT, { detail: 'touch', bubbles: true })); });
    expect(win('fares').classList.contains('is-revealed')).toBe(true);
    act(() => { vi.advanceTimersByTime(CONTROLS_REVEAL_MS + 10); });
    expect(win('fares').classList.contains('is-revealed')).toBe(false);
  });

  it('a mouse press inside the widget leaves the controls to hover', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    act(() => { win('fares').querySelector('iframe')!.dispatchEvent(new CustomEvent(APP_FRAME_TAP_EVENT, { detail: 'mouse', bubbles: true })); });
    expect(win('fares').classList.contains('is-revealed')).toBe(false);
  });

  it('a press elsewhere puts the controls away at once', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    act(() => { win('fares').querySelector('iframe')!.dispatchEvent(new CustomEvent(APP_FRAME_TAP_EVENT, { detail: 'touch', bubbles: true })); });
    act(() => { document.body.dispatchEvent(pointer('pointerdown')); });
    expect(win('fares').classList.contains('is-revealed')).toBe(false);
  });
});

describe('window mode', () => {
  const toWindow = () => act(() => win('fares').querySelector<HTMLButtonElement>('[data-widget-window-mode="window"]')!.click());

  it('switches from minimal without moving the frame in the DOM', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const frame = win('fares').querySelector('iframe');
    toWindow();
    expect(win('fares').classList.contains('is-window')).toBe(true);
    expect(win('fares').querySelector('.widget-bar')).not.toBeNull();
    expect(win('fares').querySelector('iframe')).toBe(frame);
    expect(frameProps.get('fares')?.mode).toBe('window');
  });

  it('holds its size once set, so a widget growing later never resizes it', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    toWindow();
    report('fares', { height: 200 });
    const size = widgetWindowLayout('home', k('fares')).size;
    expect(size).toBeDefined();
    const height = win('fares').style.height;
    report('fares', { height: 900 });
    expect(widgetWindowLayout('home', k('fares')).size).toEqual(size);
    expect(win('fares').style.height).toBe(height);
  });

  it('resizes from its corner, and keeps the size', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const corner = win('fares').querySelector<HTMLElement>('.widget-window-resize')!;
    corner.dispatchEvent(pointer('pointerdown', 100, 100));
    corner.dispatchEvent(pointer('pointermove', 400, 400));
    corner.dispatchEvent(pointer('pointerup', 400, 400));
    await flush();
    const size = widgetWindowLayout('home', k('fares')).size!;
    expect(size.width).toBeGreaterThan(0);
    expect(win('fares').style.width).toBe(`${size.width}px`);
    expect(win('fares').style.height).toBe(`${size.height}px`);
  });

  it('fits a size kept from a bigger screen, so its corner stays reachable', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    resizeWidgetWindow('home', k('fares'), { width: 5000, height: 5000 });
    mount();
    expect(win('fares').style.width).toBe(`${window.innerWidth - 10}px`);
    expect(win('fares').style.height).toBe(`${window.innerHeight - 20}px`);
  });

  it('comes back in its mode, size and place after a close', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    act(() => closeWidgetWindow('home', k('fares')));
    expect(shown()).toEqual([]);
    act(() => openWidgetWindow('home', k('fares'), { x: 300, y: 300 }, 24));
    expect(win('fares').classList.contains('is-window')).toBe(true);
    expect(win('fares').style.left).toBe('10px');
  });
});

describe('docked mode', () => {
  it('is how a new window opens on a phone, hung under the title row at its width', () => {
    viewportIsMobile.value = true;
    const row = document.createElement('div');
    row.className = 'mobile-thread-title-row';
    row.getBoundingClientRect = () => ({ left: 12, top: 40, right: 312, bottom: 80, width: 300, height: 40 }) as DOMRect;
    document.body.appendChild(row);
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(win('fares').classList.contains('is-docked')).toBe(true);
    expect([win('fares').style.left, win('fares').style.top, win('fares').style.width]).toEqual(['12px', '80px', '300px']);
    row.remove();
  });

  it('leaving dock returns the window to its last free place', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'docked');
    mount();
    act(() => win('fares').querySelector<HTMLButtonElement>('[data-widget-window-mode="minimal"]')!.click());
    expect([win('fares').style.left, win('fares').style.top]).toEqual(['10px', '20px']);
  });
});
