// @vitest-environment jsdom
/**
 * A widget's card in the transcript (ADR 0407, plan invariants 1 to 3). Its
 * frame mounts once on screen and takes the full reported height. Past the
 * clip box's height the card shows Expand, which opens the widget in Canvas.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { WIDGET_PARAMS_QUERY } from '@lucidos/sdk';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ThreadWidget } from '../../../api/client/widgets';

let reportOnScreen: ((onScreen: boolean) => void) | null = null;
vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (_el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    reportOnScreen = onChange;
    onChange(false);
    return () => { reportOnScreen = null; };
  },
}));

const openWidgetInCanvas = vi.fn((_appId: string, _name: string) => Promise.resolve());
vi.mock('../../../store/actions/widget-actions', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/widget-actions')>()),
  openWidgetInCanvas: (appId: string, name: string) => openWidgetInCanvas(appId, name),
}));

const { WidgetCard } = await import('../WidgetCard');
const { _resetWidgetHeightsForTesting, DEFAULT_WIDGET_HEIGHT_PX, OFF_SCREEN_HOLD_MS } = await import('../WidgetFrame');
const { threadWidgets } = await import('../../../store/widgets');
const { APP_FRAME_SIZE_EVENT } = await import('../../../store/actions/widget-frame-bridge');

const THREAD = 'thread-1';
/** A stand-in for the clip box's cap, which CSS sets and jsdom cannot read. */
const CLIP_CAP_PX = 576;

function threadWidgetsWith(widget: Partial<ThreadWidget>): void {
  threadWidgets.value = new Map([[THREAD, {
    status: 'loaded',
    data: [{
      app_id: 'flight-picker',
      name: 'Flight picker',
      reusable: false,
      reveal: 'on-load',
      pinned: false,
      shown_event_id: 'shown-1',
      ...widget,
    }],
  }]]);
}

// jsdom lays nothing out, so the clip box answers as a browser would: as tall
// as its frame, up to its cap. The stub shadows jsdom's own on `Element`.
beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      if (!this.classList.contains('widget-card-clip')) return 0;
      const frame = this.querySelector<HTMLElement>('.widget-frame');
      return Math.min(parseFloat(frame?.style.height ?? '0'), CLIP_CAP_PX);
    },
  });
});
afterEach(() => {
  delete (HTMLElement.prototype as { clientHeight?: number }).clientHeight;
});

let host: HTMLElement;

beforeEach(() => {
  _resetWidgetHeightsForTesting();
  openWidgetInCanvas.mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  threadWidgets.value = new Map();
  // A case that fails mid-way still hands the next one real timers.
  vi.useRealTimers();
});

const frames = () => host.querySelectorAll<HTMLIFrameElement>('iframe');
const frameBox = () => host.querySelector<HTMLElement>('.widget-frame')!;
const expand = () => host.querySelector<HTMLButtonElement>('.widget-card-fade .action-btn');

function renderCard(): void {
  act(() => { render(<WidgetCard threadId={THREAD} appId="flight-picker" eventId="shown-1" />, host); });
}

function report(px: number): void {
  act(() => {
    frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: px }, bubbles: true }));
  });
}

describe('WidgetCard', () => {
  it('draws the bar over a frame placeholder, with no iframe off screen', () => {
    threadWidgetsWith({});
    renderCard();
    expect(host.querySelector('.widget-card-inline')!.getAttribute('data-event-id')).toBe('shown-1');
    expect(host.querySelector('.widget-bar-name')!.textContent).toBe('Flight picker');
    expect(frameBox().style.height).toBe(`${DEFAULT_WIDGET_HEIGHT_PX}px`);
    expect(frames()).toHaveLength(0);
  });

  it('mounts the iframe on screen, and drops it once the off-screen hold runs out', () => {
    vi.useFakeTimers();
    threadWidgetsWith({});
    renderCard();
    act(() => { reportOnScreen?.(true); });
    expect(frames()).toHaveLength(1);
    act(() => { reportOnScreen?.(false); });
    expect(frames(), 'held while off screen').toHaveLength(1);
    act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS); });
    expect(frames()).toHaveLength(0);
  });

  it('gives a tall widget its full height, clipped, with Expand', () => {
    threadWidgetsWith({});
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(2000);
    expect(frameBox().style.height, 'the frame never caps, so it never scrolls').toBe('2000px');
    expect(host.querySelector('.widget-card-clip')!.contains(frameBox())).toBe(true);
    expect(expand()).not.toBeNull();
  });

  it('shows no Expand for a widget that fits', () => {
    threadWidgetsWith({});
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(300);
    expect(frameBox().style.height).toBe('300px');
    expect(expand()).toBeNull();
  });

  it('opens the widget in Canvas from Expand', () => {
    threadWidgetsWith({});
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(2000);
    act(() => { expand()!.click(); });
    expect(openWidgetInCanvas).toHaveBeenCalledWith('flight-picker', 'Flight picker');
  });

  it('keeps a tall widget clipped once its frame has unloaded', () => {
    vi.useFakeTimers();
    threadWidgetsWith({});
    renderCard();
    act(() => { reportOnScreen?.(true); });
    report(2000);
    act(() => { reportOnScreen?.(false); });
    act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS); });
    expect(frames()).toHaveLength(0);
    expect(frameBox().style.height).toBe('2000px');
    expect(expand()).not.toBeNull();
  });

  // ADR 0415: the card finds its own instance among two of one widget, puts
  // its params in the frame, and reads its label.
  it('draws its own instance: params in the frame, the label in the bar', () => {
    threadWidgets.value = new Map([[THREAD, {
      status: 'loaded',
      data: [
        { app_id: 'player', params: { clip: 'a.mp3' }, name: 'Sound player', reusable: true, reveal: 'on-load', pinned: false, shown_event_id: 's-a' },
        { app_id: 'player', params: { clip: 'b.mp3' }, name: 'Sound player', reusable: true, reveal: 'on-load', pinned: true, shown_event_id: 's-b' },
      ],
    }]]);
    act(() => {
      render(<WidgetCard threadId={THREAD} appId="player" params={{ clip: 'b.mp3' }} label="Cedar" eventId="s-b" />, host);
    });
    expect(host.querySelector('.widget-bar-name')!.textContent).toBe('Cedar');
    act(() => { reportOnScreen?.(true); });
    const src = new URL(frames()[0].src);
    expect(JSON.parse(src.searchParams.get(WIDGET_PARAMS_QUERY)!)).toEqual({ clip: 'b.mp3' });
  });

  it('is not "gone" for a legacy card with no params beside a param instance', () => {
    threadWidgetsWith({});
    renderCard();
    expect(host.querySelector('.widget-card-gone')).toBeNull();
  });
});

