// @vitest-environment jsdom
/**
 * A widget's frame IS an app frame (ADR 0402, plan invariant 1). An inline one
 * fills its widget box, mounts once it comes on screen and stays a while after
 * it scrolls off. An option one takes its widget's height.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { WIDGET_PARAMS_QUERY } from '@lucidos/sdk';
import { render } from 'preact';
import { act } from 'preact/test-utils';

let reportOnScreen: ((onScreen: boolean) => void) | null = null;
vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (_el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    reportOnScreen = onChange;
    onChange(false);
    return () => { reportOnScreen = null; };
  },
}));

import { WidgetFrame, OPTION_WIDGET_HEIGHT_PX, OFF_SCREEN_HOLD_MS, _resetWidgetHeightsForTesting } from '../WidgetFrame';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from '../../apps/appFrameSandbox';
import { APP_FRAME_SIZE_EVENT } from '../../../store/actions/widget-frame-bridge';
import { appFrameFor, appIdForFrame, isCanvasAppFrame } from '../../../utils/appFrame';
import { reloadWidgetFrames, widgetRefreshKeys } from '../../../store/widgets';

let host: HTMLElement;

beforeEach(() => {
  _resetWidgetHeightsForTesting();
  widgetRefreshKeys.value = new Map();
  // A frame applies a report on the next animation frame. Run it at once.
  vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => { run(performance.now()); return 0; });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.unstubAllGlobals();
  render(null, host);
  host.remove();
});

const frames = () => host.querySelectorAll<HTMLIFrameElement>('iframe');
const frameBox = () => host.querySelector<HTMLElement>('.widget-frame')!;

describe('WidgetFrame', () => {
  it('holds no inline iframe before it first comes on screen', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" size="medium" />, host); });
    expect(frames()).toHaveLength(0);
    expect(frameBox().classList.contains('widget-frame-inline')).toBe(true);

    act(() => { reportOnScreen?.(true); });
    expect(frames()).toHaveLength(1);
  });

  describe('off screen', () => {
    beforeEach(() => { vi.useFakeTimers(); });
    afterEach(() => { vi.useRealTimers(); });

    function seenThenScrolledOff(): HTMLIFrameElement {
      act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" size="medium" />, host); });
      act(() => { reportOnScreen?.(true); });
      const frame = frames()[0];
      act(() => { reportOnScreen?.(false); });
      return frame;
    }

    it('keeps the frame loaded through the hold', () => {
      const frame = seenThenScrolledOff();
      act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS - 1); });
      expect(frames()[0], 'the same frame, never reloaded').toBe(frame);
    });

    it('unloads the frame once the hold runs out', () => {
      seenThenScrolledOff();
      act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS); });
      expect(frames()).toHaveLength(0);
    });

    it('keeps the frame when the reader scrolls back inside the hold', () => {
      const frame = seenThenScrolledOff();
      act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS - 1); });
      act(() => { reportOnScreen?.(true); });
      act(() => { vi.advanceTimersByTime(OFF_SCREEN_HOLD_MS); });
      expect(frames()[0]).toBe(frame);
    });
  });

  it('draws the app frame itself: same role, sandbox, allow and app id', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, host); });
    act(() => { reportOnScreen?.(true); });
    const frame = frames()[0];
    expect(frame.getAttribute('data-role')).toBe('app-ui-frame');
    expect(frame.getAttribute('sandbox')).toBe(APP_FRAME_SANDBOX);
    expect(frame.getAttribute('allow')).toBe(APP_FRAME_ALLOW);
    expect(appIdForFrame(frame)).toBe('fare-grid');
    expect(isCanvasAppFrame(frame), 'a widget is never the Canvas app').toBe(false);
    // The bridge answers only a frame it finds this way, so a widget's
    // `lucidos.request` and `lucidos.sse` pass exactly as an app's do.
    expect(appFrameFor(frame.contentWindow)).toBe(frame);
  });

  it('sizes to the height the widget reports, and keeps it as a remount\'s placeholder', () => {
    const heights: number[] = [];
    act(() => {
      render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" onHeight={(px) => heights.push(px)} />, host);
    });
    act(() => { reportOnScreen?.(true); });
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 312 }, bubbles: true }));
    });
    expect(frameBox().style.height).toBe('312px');
    expect(heights).toEqual([OPTION_WIDGET_HEIGHT_PX, 312]);

    act(() => { render(null, host); });
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, host); });
    expect(frames(), 'off screen after the remount').toHaveLength(0);
    expect(frameBox().style.height).toBe('312px');
  });

  it('locks a frame whose content creeps after its height, and never remembers the runaway', () => {
    const report = (px: number) => frames()[0].dispatchEvent(
      new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: px }, bubbles: true }),
    );
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, host); });
    act(() => { reportOnScreen?.(true); });
    // A `100vh` body with an 8px margin each side reports 16px past the frame.
    for (let px = OPTION_WIDGET_HEIGHT_PX + 16; px < 1200; px += 16) act(() => { report(px); });
    expect(frameBox().style.height, 'back where the creep began').toBe(`${OPTION_WIDGET_HEIGHT_PX}px`);

    act(() => { render(null, host); });
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, host); });
    act(() => { reportOnScreen?.(true); });
    expect(frameBox().style.height).toBe(`${OPTION_WIDGET_HEIGHT_PX}px`);
  });

  it('releases a lock when the widget reloads, so a fixed widget sizes again', () => {
    const report = (px: number) => frames()[0].dispatchEvent(
      new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: px }, bubbles: true }),
    );
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, host); });
    act(() => { reportOnScreen?.(true); });
    for (let px = OPTION_WIDGET_HEIGHT_PX + 16; px < 1200; px += 16) act(() => { report(px); });
    act(() => { report(312); });
    expect(frameBox().style.height, 'still locked').toBe(`${OPTION_WIDGET_HEIGHT_PX}px`);

    act(() => { reloadWidgetFrames('fare-grid'); });
    act(() => { report(312); });
    expect(frameBox().style.height).toBe('312px');
  });

  it('carries its params in the src, and remembers a height per instance', () => {
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'a.mp3' }} reveal="on-load" place="option" />, host);
    });
    act(() => { reportOnScreen?.(true); });
    const src = new URL(frames()[0].src);
    expect(JSON.parse(src.searchParams.get(WIDGET_PARAMS_QUERY)!)).toEqual({ clip: 'a.mp3' });
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 96 }, bubbles: true }));
    });

    act(() => { render(null, host); });
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'b.mp3' }} reveal="on-load" place="option" />, host);
    });
    act(() => { reportOnScreen?.(true); });
    expect(frameBox().style.height, 'another instance starts at the default').toBe(`${OPTION_WIDGET_HEIGHT_PX}px`);

    act(() => { render(null, host); });
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'a.mp3' }} reveal="on-load" place="option" />, host);
    });
    act(() => { reportOnScreen?.(true); });
    expect(frameBox().style.height).toBe('96px');
  });

  it('fills the box of an inline frame given a size, and a report only feeds the fade (I4)', () => {
    const sizes: Array<[number, boolean]> = [];
    act(() => {
      render(
        <WidgetFrame appId="fare-grid" reveal="on-load" place="inline" size="medium" onSize={(s, fromReport) => sizes.push([s.height, fromReport])} />,
        host,
      );
    });
    act(() => { reportOnScreen?.(true); });
    expect(frameBox().style.height, 'the box sets the height, never the frame').toBe('');

    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 900 }, bubbles: true }));
    });
    expect(frameBox().style.height).toBe('');
    expect(sizes[sizes.length - 1]).toEqual([900, true]);
  });
});

/** A transcript whose top sits at y=100, scrolled to `scrollTop`. A box laid
 *  out at `offset` in its content measures from that, `height` tall. jsdom
 *  lays nothing out, so the rects are drawn by hand. */
function transcript(scrollTop: number) {
  const scroller = document.createElement('div');
  scroller.className = 'thread-content';
  let top = scrollTop;
  Object.defineProperty(scroller, 'scrollTop', { get: () => top, set: (v: number) => { top = v; } });
  scroller.getBoundingClientRect = () => new DOMRect(0, 100, 400, 600);
  const layOut = (el: HTMLElement, offset: number, height: () => number) => {
    el.getBoundingClientRect = () => new DOMRect(0, 100 + offset - top, 400, height());
  };
  return { scroller, layOut };
}

describe('WidgetFrame in the transcript', () => {
  const report = (px: number) => act(() => {
    frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: px }, bubbles: true }));
  });
  const boxHeight = () => parseFloat(frameBox().style.height);

  it('holds the reader still when a widget crossing their line changes height', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    // Its top is 100px above the line, so the reader reads what follows it.
    layOut(frameBox(), 200, boxHeight);

    report(600);
    expect(scroller.scrollTop, 'grown by 552px above the reader').toBe(852);
    report(80);
    expect(scroller.scrollTop, 'shrunk by 520px above the reader').toBe(332);
    render(null, scroller);
  });

  it('leaves a widget the reader is using where it is', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    // Across the reader's line, so the reader is using it in view.
    layOut(frameBox(), 280, boxHeight);
    // A tap inside the widget focuses its frame.
    frames()[0].tabIndex = 0;
    frames()[0].focus();

    report(600);
    expect(scroller.scrollTop).toBe(300);
    render(null, scroller);
  });

  it('holds the reader still for a widget they used that has scrolled above their line', () => {
    // A frame stays loaded off screen, and scrolling never blurs it.
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    frames()[0].tabIndex = 0;
    frames()[0].focus();
    // Its bottom is 60px above the line, so it is wholly off screen.
    layOut(frameBox(), 0, boxHeight);

    report(600);
    expect(scroller.scrollTop, 'grown by 552px above the reader').toBe(852);
    render(null, scroller);
  });

  it('leaves the reader alone when nothing after the widget is on screen', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    // 100px above the line and 800px tall, so it runs past the 600px band.
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 800 }, bubbles: true }));
    });
    layOut(frameBox(), 200, boxHeight);

    report(900);
    expect(scroller.scrollTop).toBe(300);
    render(null, scroller);
  });

  it('leaves the reader alone when the widget sits below their line', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="option" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    layOut(frameBox(), 500, boxHeight);

    report(600);
    expect(scroller.scrollTop).toBe(300);
    render(null, scroller);
  });
});
