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
const frameProps = new Map<string, { mode?: string; draggable?: boolean; onSize?: OnSize }>();
vi.mock('../WidgetFrame', () => ({
  WidgetFrame: (props: { appId: string; mode?: string; draggable?: boolean; onSize?: OnSize }) => {
    frameProps.set(props.appId, props);
    return <iframe data-frame={props.appId} />;
  },
}));

import type { ThreadWidget } from '../../../api/client/widgets';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { APP_FRAME_DRAG_EVENT, APP_FRAME_TAP_EVENT } from '../../../store/actions/widget-frame-bridge';
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
import { getRemPx } from '../../../utils/dom';
import { DOCK_GAP_REM } from '../useDockLine';
import { CONTROLS_REVEAL_MS, CONTROLS_ROOM_REM, controlsBelow, WidgetWindows, WIDGET_WINDOW_FADE_MS } from '../WidgetWindows';

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
/** Picks a mode the way the user does: the mode button, then its menu row. */
const pickMode = (appId: string, mode: string) => {
  act(() => win(appId).querySelector<HTMLButtonElement>('.widget-window-mode')!.click());
  act(() => document.querySelector<HTMLButtonElement>(`[role="menuitemradio"][data-widget-window-mode="${mode}"]`)!.click());
};
/** Every control a window draws: its own buttons and menu triggers. */
const controls = (appId: string) => [...win(appId).querySelectorAll<HTMLButtonElement>('button')]
  .map((b) => b.getAttribute('aria-label'));
/** A drag step the widget's frame passes up, in screen pixels. */
const frameDrag = (appId: string, phase: string, screenX: number, screenY: number) => act(() => {
  win(appId).querySelector('iframe')!.dispatchEvent(
    new CustomEvent(APP_FRAME_DRAG_EVENT, { detail: { phase, screenX, screenY }, bubbles: true }),
  );
});
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

  it('a cancelled bar drag stays where the pointer last was', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const bar = win('fares').querySelector<HTMLElement>('.widget-bar-name')!;
    bar.dispatchEvent(pointer('pointerdown', 100, 100));
    bar.dispatchEvent(pointer('pointermove', 150, 180));
    bar.dispatchEvent(pointer('pointercancel', 0, 0));
    await flush();
    expect(placed('fares')).toEqual([60, 100]);
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

  it('drags by its controls pill, between the buttons', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    const pill = win('fares').querySelector<HTMLElement>('.widget-window-controls')!;
    pill.dispatchEvent(pointer('pointerdown', 100, 100));
    pill.dispatchEvent(pointer('pointermove', 150, 180));
    pill.dispatchEvent(pointer('pointerup', 150, 180));
    await flush();
    expect(placed('fares')).toEqual([60, 100]);
  });

  it('floats three controls: the mode, the widget menu and Close', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(controls('fares')).toEqual(['Mode: Minimal', 'fares widget actions', 'Close fares']);
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
  const toWindow = () => pickMode('fares', 'window');

  it('draws its name and three controls in its bar, and nothing else', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    expect(controls('fares')).toEqual(['Mode: Window', 'fares widget actions', 'Close fares']);
    expect(win('fares').querySelector('.widget-bar-kind')).toBeNull();
  });

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

  it('resizes from its bottom right corner, and keeps the size', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const corner = win('fares').querySelector<HTMLElement>('.widget-window-resize[data-corner="se"]')!;
    corner.dispatchEvent(pointer('pointerdown', 100, 100));
    corner.dispatchEvent(pointer('pointermove', 400, 400));
    corner.dispatchEvent(pointer('pointerup', 400, 400));
    await flush();
    const size = widgetWindowLayout('home', k('fares')).size!;
    expect(size.width).toBeGreaterThan(0);
    expect(win('fares').style.width).toBe(`${size.width}px`);
    expect(win('fares').style.height).toBe(`${size.height}px`);
  });

  it('resizes from its top left corner, keeping the bottom right one in place', async () => {
    openWidgetWindow('home', k('fares'), { x: 100, y: 120 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    resizeWidgetWindow('home', k('fares'), { width: 400, height: 300 });
    mount();
    const corner = win('fares').querySelector<HTMLElement>('.widget-window-resize[data-corner="nw"]')!;
    corner.dispatchEvent(pointer('pointerdown', 100, 120));
    corner.dispatchEvent(pointer('pointermove', 60, 90));
    corner.dispatchEvent(pointer('pointerup', 60, 90));
    await flush();
    expect(placed('fares')).toEqual([60, 90]);
    expect(widgetWindowLayout('home', k('fares')).size).toEqual({ width: 440, height: 330 });
  });

  it('draws a grab zone at each corner and no mark', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'window');
    mount();
    const zones = [...win('fares').querySelectorAll<HTMLElement>('.widget-window-resize')];
    expect(zones.map((z) => z.dataset.corner)).toEqual(['nw', 'ne', 'sw', 'se']);
    expect(zones.every((z) => z.childNodes.length === 0)).toBe(true);
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
  /** A shell element at a fixed box, as jsdom lays out nothing. */
  const shellAt = (className: string, rect: { left: number; top: number; right: number; bottom: number }) => {
    const el = document.createElement('div');
    el.className = className;
    el.getBoundingClientRect = () => ({ ...rect, width: rect.right - rect.left, height: rect.bottom - rect.top }) as DOMRect;
    document.body.appendChild(el);
    return el;
  };
  const gap = () => DOCK_GAP_REM * getRemPx();

  it('is how a new window opens on a phone, hung under the title row and inside it', () => {
    viewportIsMobile.value = true;
    const row = shellAt('mobile-thread-title-row', { left: 12, top: 40, right: 312, bottom: 80 });
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(win('fares').classList.contains('is-docked')).toBe(true);
    expect([win('fares').style.left, win('fares').style.top]).toEqual([`${12 + gap()}px`, 'calc(80px)']);
    expect(win('fares').style.maxWidth).toBe(`${300 - 2 * gap()}px`);
    row.remove();
  });

  it('hides on a phone while the title row is off screen, as after a swipe to another pane', () => {
    viewportIsMobile.value = true;
    vi.stubGlobal('IntersectionObserver', class {
      constructor(private readonly onChange: (entries: { isIntersecting: boolean }[]) => void) {}
      observe() { this.onChange([{ isIntersecting: false }]); }
      disconnect() {}
    });
    const row = shellAt('mobile-thread-title-row', { left: 400, top: 40, right: 700, bottom: 80 });
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(win('fares').classList.contains('is-dock-hidden')).toBe(true);
    row.remove();
  });

  it('hangs under the app header and its banners on a desktop, under its own place and at its own width', () => {
    const titleRow = shellAt('thread-view-header', { left: 300, top: 48, right: 1000, bottom: 96 });
    openWidgetWindow('home', k('fares'), { x: 100, y: 300 }, 24);
    resizeWidgetWindow('home', k('fares'), { width: 300, height: 200 });
    setWidgetWindowMode('home', k('fares'), 'docked');
    mount();
    expect([win('fares').style.left, win('fares').style.top, win('fares').style.width])
      .toEqual(['100px', 'calc(var(--app-header-bottom) + 0px)', '300px']);
    titleRow.remove();
  });

  it('leaving dock returns the window to its last free place', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'docked');
    mount();
    pickMode('fares', 'minimal');
    expect([win('fares').style.left, win('fares').style.top]).toEqual(['10px', '20px']);
  });

  it('floats the same three controls as minimal', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'docked');
    mount();
    expect(controls('fares')).toEqual(['Mode: Dock under the header', 'fares widget actions', 'Close fares']);
  });
});

describe('a drag from inside the widget', () => {
  it('moves the window as a bar drag would, and keeps the place', async () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    frameDrag('fares', 'start', 500, 500);
    frameDrag('fares', 'move', 550, 580);
    expect(win('fares').style.left).toBe('60px');
    expect(win('fares').classList.contains('is-frame-dragging')).toBe(true);
    frameDrag('fares', 'end', 550, 580);
    expect(placed('fares')).toEqual([60, 100]);
    expect(win('fares').classList.contains('is-frame-dragging')).toBe(false);
  });

  it('ends inside the screen, as a bar drag does', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    frameDrag('fares', 'start', 500, 500);
    frameDrag('fares', 'end', -5000, -5000);
    const [x, y] = placed('fares');
    expect(x).toBeGreaterThan(-5000);
    expect(y).toBeGreaterThanOrEqual(0);
  });

  it('raises the window it drags', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    openWidgetWindow('home', k('weather'), { x: 10, y: 20 }, 24);
    mount();
    frameDrag('fares', 'start', 500, 500);
    expect(win('fares').style.zIndex).toBe('calc(var(--z-widget-window) + 1)');
  });

  it('tells the frame only a window that moves may drag, never a docked one', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    mount();
    expect(frameProps.get('fares')?.draggable).toBe(true);
    act(() => setWidgetWindowMode('home', k('fares'), 'docked'));
    expect(frameProps.get('fares')?.draggable).toBe(false);
    frameDrag('fares', 'start', 500, 500);
    frameDrag('fares', 'end', 550, 580);
    expect(placed('fares')).toEqual([10, 20]);
  });
});

describe('the mode button', () => {
  it('shows the current mode, and its menu checks it among all three', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 20 }, 24);
    setWidgetWindowMode('home', k('fares'), 'docked');
    mount();
    const button = win('fares').querySelector<HTMLButtonElement>('.widget-window-mode')!;
    expect(button.getAttribute('aria-label')).toBe('Mode: Dock under the header');
    act(() => button.click());
    const rows = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')];
    expect(rows.map((r) => [r.dataset.widgetWindowMode, r.getAttribute('aria-checked')]))
      .toEqual([['minimal', 'false'], ['window', 'false'], ['docked', 'true']]);
    act(() => rows[0].click());
    expect(widgetWindowLayout('home', k('fares')).mode).toBe('minimal');
  });
});

describe('the floating controls', () => {
  it('sit over the widget, or under it with no room above or when docked', () => {
    const room = CONTROLS_ROOM_REM * getRemPx();
    expect(controlsBelow('minimal', room)).toBe(false);
    expect(controlsBelow('minimal', room - 1)).toBe(true);
    expect(controlsBelow('docked', 500)).toBe(true);
  });

  it('take the side the window has room for', () => {
    openWidgetWindow('home', k('fares'), { x: 10, y: 0 }, 24);
    mount();
    expect(win('fares').querySelector('.widget-window-controls')!.classList.contains('is-below')).toBe(true);
  });
});
