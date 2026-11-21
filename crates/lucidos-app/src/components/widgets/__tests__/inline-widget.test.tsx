// @vitest-environment jsdom
/**
 * An inline widget in the transcript (ADRs 0432, 0466): a bare widget box,
 * the box a dashboard tile is. Its frame mounts once on screen and fills the
 * box. Content past the box is clipped under a fade. The widget menu's size
 * row picks another declared size and keeps the frame. A reply's embed and an
 * old `WidgetShown` draw the same one.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { WIDGET_PARAMS_QUERY } from '@lucidos/sdk';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { App } from '../../../store/types';
import { widgetBoxCss } from '../../../utils/dashboardGeometry';
import type { StoredEvent } from '../../../store/thread-events';

let reportOnScreen: ((onScreen: boolean) => void) | null = null;
vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (_el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    reportOnScreen = onChange;
    onChange(false);
    return () => { reportOnScreen = null; };
  },
}));

const addToDashboard = vi.fn((..._args: unknown[]) => Promise.resolve());
vi.mock('../../../store/actions/dashboards', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/dashboards')>()),
  addToDashboard: (...args: unknown[]) => addToDashboard(...args),
  rereadDashboard: () => Promise.resolve(),
}));

/** A pick lands as the engine's `InlineWidgetSized`, which the card draws. */
let nextSeq = 1;
function sizeEventArrives(appId: string, size: string): void {
  const thread = threadMap.value.get(THREAD);
  const events = new Map(thread?.events);
  events.set(nextSeq++, { type: 'InlineWidgetSized', app_id: appId, size } as StoredEvent);
  // Only `events` is read, so the rest of the thread stays a stub.
  threadMap.value = new Map([[THREAD, { ...thread, events } as never]]);
}
const sizeWidgetCards = vi.fn((instance: { appId: string }, _name: string, size: string) => {
  sizeEventArrives(instance.appId, size);
  return Promise.resolve();
});
vi.mock('../../../store/actions/widget-actions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/widget-actions')>()),
  sizeWidgetCards: (...args: Parameters<typeof sizeWidgetCards>) => sizeWidgetCards(...args),
}));

const { InlineWidget, WIDGET_UNAVAILABLE_TEXT } = await import('../InlineWidget');
const { threadMap } = await import('../../../store/store');
const { _resetWidgetHeightsForTesting, OFF_SCREEN_HOLD_MS } = await import('../WidgetFrame');
const { appsById } = await import('../../../store/appsById');
const { dashboards } = await import('../../../store/dashboards');
const { APP_FRAME_MENU_EVENT, APP_FRAME_SIZE_EVENT } = await import('../../../store/actions/widget-frame-bridge');
const { WidgetMenuItems } = await import('../widgetMenuChunks');
const { LONG_PRESS_DELAY_MS } = await import('../../../hooks/useLongPress');

const THREAD = 'thread-1';
/** A stand-in for the widget box's height, which CSS sets and jsdom cannot read. */
const BOX_PX = 160;

function readEmptyDashboard(): void {
  dashboards.value = new Map([['desktop', { status: 'loaded', data: { dashboard: 'desktop', tiles: [] } }]]);
}

function appRead(app: Partial<App> & { id: string }): void {
  appsById.value = new Map([...appsById.value, [app.id, {
    status: 'loaded',
    data: { name: app.id, description: '', reveal: 'on-load', kind: 'widget', reusable: false, sizes: ['medium', 'small', 'large'], ...app } as App,
  }]]);
}

// jsdom lays nothing out, so the widget box answers as a browser would: as
// tall as its size. The stub shadows jsdom's own on `Element`.
beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains('widget-box') ? BOX_PX : 0;
    },
  });
});
afterEach(() => {
  delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
});

let host: HTMLElement;

beforeEach(async () => {
  // The menu opens once its rows are in memory: load them up front.
  await WidgetMenuItems.preload();
  _resetWidgetHeightsForTesting();
  // A frame applies a report on the next animation frame. Run it at once.
  vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => { run(performance.now()); return 0; });
  appRead({ id: 'flight-picker', name: 'Flight picker' });
  appRead({ id: 'player', name: 'Sound player', reusable: true });
  addToDashboard.mockClear();
  sizeWidgetCards.mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.unstubAllGlobals();
  render(null, host);
  host.remove();
  appsById.value = new Map();
  dashboards.value = new Map();
  threadMap.value = new Map();
  // A case that fails mid-way still hands the next one real timers.
  vi.useRealTimers();
});

const frames = () => host.querySelectorAll<HTMLIFrameElement>('iframe');
const box = () => host.querySelector<HTMLElement>('.inline-widget .widget-box')!;
const fade = () => host.querySelector('.widget-box-fade');
const card = () => host.querySelector<HTMLElement>('.inline-widget')!;
// The menu is portalled, so its rows are read from the document.
const sizeOption = (size: string) => document.querySelector<HTMLButtonElement>(`.thread-overflow-menu [role="radio"][aria-label="${size}"]`)!;
const menuItem = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>('.thread-overflow-menu [role="menuitem"]')].find((b) => b.textContent!.startsWith(label));
const menuItems = () => [...document.querySelectorAll('.thread-overflow-menu [role="menuitem"]')].map((b) => b.textContent);
const rightClick = (target: Element, init: MouseEventInit = {}) => {
  const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 30, clientY: 40, ...init });
  act(() => { target.dispatchEvent(e); });
  return e;
};
const openMenu = () => rightClick(card());

function renderCard(): void {
  act(() => { render(<InlineWidget threadId={THREAD} appId="flight-picker" eventId="shown-1" stamped />, host); });
}

function report(px: number): void {
  act(() => {
    frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: px }, bubbles: true }));
  });
}

describe('InlineWidget', () => {
  it('draws its bare box at the default size, with no bar and no iframe off screen', () => {
    renderCard();
    expect(card().getAttribute('data-event-id')).toBe('shown-1');
    expect(host.querySelector('.widget-bar')).toBeNull();
    expect(card().getAttribute('aria-label')).toBe('Flight picker');
    expect(box().getAttribute('data-size'), 'the first declared size').toBe('medium');
    expect(frames()).toHaveLength(0);
  });

  it('sets the box from its size, on the card', () => {
    renderCard();
    expect(card().style.getPropertyValue('--widget-box-width')).toContain('2 * var(--widget-cell)');
    expect(card().style.getPropertyValue('--widget-box-height')).toContain('1 * var(--widget-cell)');
  });

  it('mounts the iframe on screen, and drops it once the off-screen hold runs out', () => {
    vi.useFakeTimers();
    renderCard();
    act(() => { reportOnScreen?.(true); });
    expect(frames()).toHaveLength(1);
    act(() => { reportOnScreen?.(false); });
    expect(frames(), 'held while off screen').toHaveLength(1);
    act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS); });
    expect(frames()).toHaveLength(0);
  });

  it('clips a tall widget under a fade, and never sizes the box from a report (I4)', () => {
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(2000);
    expect(box().querySelector<HTMLElement>('.widget-frame')!.style.height).toBe('');
    expect(fade()).not.toBeNull();
  });

  it('shows no fade for a widget that fits', () => {
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(120);
    expect(fade()).toBeNull();
  });

  it("its menu's size row offers the declared sizes, and a pick resizes the box with the same frame (I12)", () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: ['medium', 'large'] });
    renderCard();
    act(() => { reportOnScreen?.(true); });
    const frame = frames()[0];
    openMenu();
    expect(document.querySelector('.widget-menu-size')!.textContent).toMatch(/^Size/);
    expect([...document.querySelectorAll('.thread-overflow-menu [role="radio"]')].map((b) => b.getAttribute('aria-label'))).toEqual(['medium', 'large']);
    expect(sizeOption('medium').getAttribute('aria-checked')).toBe('true');

    act(() => { sizeOption('large').click(); });
    expect(sizeWidgetCards).toHaveBeenCalledWith({ appId: 'flight-picker', threadId: THREAD, params: undefined }, 'Flight picker', 'large');
    expect(box().getAttribute('data-size')).toBe('large');
    expect(sizeOption('large').getAttribute('aria-checked'), 'the menu stays open on the new size').toBe('true');
    expect(frames()[0], 'the frame stays').toBe(frame);
  });

  /** Invariant: a card too narrow for XL is clamped by CSS and the box keeps
   *  the true XL width in its scroller. The widget still gets `xl`. */
  it('draws an XL card at four cells, the box holding that width in a scroller', () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: ['medium', 'large', 'xl'] });
    renderCard();
    openMenu();
    act(() => { sizeOption('xl').click(); });
    expect(box().getAttribute('data-size')).toBe('xl');
    expect(card().classList.contains('inline-widget-xl')).toBe(true);
    expect(card().style.getPropertyValue('--widget-box-width')).toBe(widgetBoxCss('xl').width);
    expect(card().style.getPropertyValue('--widget-box-height')).toBe(widgetBoxCss('xl').height);
    const canvas = box().querySelector<HTMLElement>(':scope > .widget-box-scroll > .widget-box-canvas')!;
    expect(canvas.style.getPropertyValue('--widget-box-true-width')).toBe(widgetBoxCss('xl').width);
  });

  it('offers no size row for a widget with one size', () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: ['large'] });
    renderCard();
    openMenu();
    expect(menuItems().length).toBeGreaterThan(0);
    expect(document.querySelector('.widget-menu-size')).toBeNull();
    expect(box().getAttribute('data-size')).toBe('large');
  });

  it("adds the widget to the dashboard from its menu, once the dashboard is read", () => {
    renderCard();
    openMenu();
    expect(menuItem('Add to dashboard')!.getAttribute('aria-disabled'), 'not until the dashboard is read').toBe('true');
    act(() => { readEmptyDashboard(); });
    act(() => { menuItem('Add to dashboard')!.click(); });
    expect(addToDashboard).toHaveBeenCalledWith('desktop', { appId: 'flight-picker', params: undefined, label: undefined, size: 'medium' }, 'Flight picker', 'widget');
  });

  // I6 and I7: the card draws the thread's newest pick, as after a reload or
  // a pick on another device, and adds its tile at that size.
  it('draws the newest size the thread holds, and adds at it', () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: ['medium', 'large'] });
    readEmptyDashboard();
    sizeEventArrives('player', 'large');
    sizeEventArrives('flight-picker', 'large');
    renderCard();
    expect(box().getAttribute('data-size')).toBe('large');
    act(() => { sizeEventArrives('flight-picker', 'medium'); });
    expect(box().getAttribute('data-size')).toBe('medium');
    act(() => { sizeEventArrives('flight-picker', 'large'); });
    openMenu();
    act(() => { menuItem('Add to dashboard')!.click(); });
    expect(addToDashboard).toHaveBeenCalledWith('desktop', expect.objectContaining({ size: 'large' }), 'Flight picker', 'widget');
  });

  it('starts at the default size when the newest pick is one it no longer declares', () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: ['medium', 'large'] });
    sizeEventArrives('flight-picker', 'small');
    renderCard();
    expect(box().getAttribute('data-size')).toBe('medium');
  });

  // ADR 0415: the card finds its own instance among two of one widget, puts
  // its params in the frame, and reads its label.
  it('draws its own instance: params in the frame, the label as its name', () => {
    act(() => {
      render(<InlineWidget threadId={THREAD} appId="player" params={{ clip: 'b.mp3' }} label="Cedar" eventId="s-b" stamped />, host);
    });
    expect(card().getAttribute('aria-label')).toBe('Cedar');
    act(() => { reportOnScreen?.(true); });
    const src = new URL(frames()[0].src);
    expect(JSON.parse(src.searchParams.get(WIDGET_PARAMS_QUERY)!)).toEqual({ clip: 'b.mp3' });
  });

  it("names itself from the widget's manifest", () => {
    renderCard();
    expect(card().getAttribute('aria-label')).toBe('Flight picker');
  });

  it("wears no event id of its own in a reply: the turn root wears the starter", () => {
    act(() => { render(<InlineWidget threadId={THREAD} appId="flight-picker" eventId="starter-1" />, host); });
    expect(host.querySelector('.inline-widget')!.hasAttribute('data-event-id')).toBe(false);
  });

  it('says it is not available for a widget that declares no sizes', () => {
    appRead({ id: 'flight-picker', name: 'Flight picker', sizes: [] });
    renderCard();
    expect(host.querySelector('.inline-widget-gone-reason')!.textContent).toBe("Widget 'flight-picker' declares no sizes.");
  });

  it('says it is not available, and why, for a missing widget or a refused reply embed', () => {
    appsById.value = new Map([...appsById.value, ['gone', { status: 'loaded', data: null }]]);
    act(() => { render(<InlineWidget threadId={THREAD} appId="gone" />, host); });
    expect(host.textContent).toBe(`${WIDGET_UNAVAILABLE_TEXT}No widget 'gone' exists.`);
    act(() => { render(<InlineWidget threadId={THREAD} appId="flight-picker" refusal={() => 'Because.'} />, host); });
    expect(host.querySelector('.inline-widget-gone-reason')!.textContent).toBe('Because.');
    expect(frames()).toHaveLength(0);
  });
});


describe('InlineWidget menu', () => {
  beforeEach(renderCard);

  it("a right-click on the card opens the widget's own menu, with no ⋯ drawn", () => {
    expect(rightClick(box()).defaultPrevented).toBe(true);
    expect(menuItems()).toEqual(expect.arrayContaining(['Open in CanvasFull size, beside the thread']));
    expect(host.querySelector('button[aria-label="Flight picker widget actions"]')).toBeNull();
  });

  it('Option+right-click leaves the native menu', () => {
    expect(rightClick(box(), { altKey: true }).defaultPrevented).toBe(false);
    expect(menuItems()).toEqual([]);
  });

  it.each([
    ['the menu key', { key: 'ContextMenu' }],
    ['Shift+F10', { key: 'F10', shiftKey: true }],
  ])('the card is a focus stop, and %s opens the menu', (_name, init) => {
    expect(card().tabIndex).toBe(0);
    const e = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
    act(() => { card().dispatchEvent(e); });
    expect(e.defaultPrevented).toBe(true);
    expect(menuItems().length).toBeGreaterThan(0);
  });

  it('another key on the card opens no menu', () => {
    act(() => { card().dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, key: 'F10' })); });
    expect(menuItems()).toEqual([]);
  });

  it("the frame's forwarded right-click opens the same menu", () => {
    act(() => { reportOnScreen?.(true); });
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { detail: { x: 30, y: 40 }, bubbles: true }));
    });
    expect(menuItems().length).toBeGreaterThan(0);
  });

  it('a hold on the card opens the menu, and the widget does not also take its click', () => {
    vi.useFakeTimers();
    const target = box();
    const clicked = vi.fn();
    target.addEventListener('click', clicked);
    act(() => {
      target.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
      vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    });
    expect(menuItems().length).toBeGreaterThan(0);
    // Closed first, so only the card can stop the click that follows.
    act(() => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(menuItems()).toEqual([]);
    // The lift's paired click is swallowed before it reaches the box.
    act(() => {
      target.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, button: 0 }));
      target.click();
    });
    expect(clicked).not.toHaveBeenCalled();
  });
});
