// @vitest-environment jsdom
/**
 * A widget's frame IS an app frame (ADR 0402, plan invariant 1). An inline one
 * mounts once it comes on screen and stays a while after it scrolls off; a
 * shelf one mounts at once.
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

import { WidgetFrame, DEFAULT_WIDGET_HEIGHT_PX, OFF_SCREEN_HOLD_MS, _resetWidgetHeightsForTesting } from '../WidgetFrame';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from '../../apps/appFrameSandbox';
import { APP_FRAME_SIZE_EVENT } from '../../../store/actions/widget-frame-bridge';
import { appFrameFor, appIdForFrame, isCanvasAppFrame } from '../../../utils/appFrame';

let host: HTMLElement;

beforeEach(() => {
  _resetWidgetHeightsForTesting();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const frames = () => host.querySelectorAll<HTMLIFrameElement>('iframe');
const frameBox = () => host.querySelector<HTMLElement>('.widget-frame')!;

describe('WidgetFrame', () => {
  it('mounts a shelf frame at once, at the default height', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" />, host); });
    expect(frames()).toHaveLength(1);
    expect(frameBox().classList.contains('widget-frame-shelf')).toBe(true);
    expect(frameBox().style.height).toBe(`${DEFAULT_WIDGET_HEIGHT_PX}px`);
  });

  it('holds no inline iframe before it first comes on screen', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" />, host); });
    expect(frames()).toHaveLength(0);
    expect(frameBox().classList.contains('widget-frame-inline')).toBe(true);
    expect(frameBox().style.height).toBe(`${DEFAULT_WIDGET_HEIGHT_PX}px`);

    act(() => { reportOnScreen?.(true); });
    expect(frames()).toHaveLength(1);
  });

  describe('off screen', () => {
    beforeEach(() => { vi.useFakeTimers(); });
    afterEach(() => { vi.useRealTimers(); });

    function seenThenScrolledOff(): HTMLIFrameElement {
      act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" />, host); });
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
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" />, host); });
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

  it('sizes to the height the widget reports, and keeps it as the inline placeholder', () => {
    const heights: number[] = [];
    act(() => {
      render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" onHeight={(px) => heights.push(px)} />, host);
    });
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 312 }, bubbles: true }));
    });
    expect(frameBox().style.height).toBe('312px');
    expect(heights).toEqual([DEFAULT_WIDGET_HEIGHT_PX, 312]);

    act(() => { render(null, host); });
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" />, host); });
    expect(frames(), 'off screen after the remount').toHaveLength(0);
    expect(frameBox().style.height).toBe('312px');
  });

  it('carries its params in the src, and remembers a height per instance', () => {
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'a.mp3' }} reveal="on-load" place="shelf" />, host);
    });
    const src = new URL(frames()[0].src);
    expect(JSON.parse(src.searchParams.get(WIDGET_PARAMS_QUERY)!)).toEqual({ clip: 'a.mp3' });
    act(() => {
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 48 }, bubbles: true }));
    });

    act(() => { render(null, host); });
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'b.mp3' }} reveal="on-load" place="shelf" />, host);
    });
    expect(frameBox().style.height, 'another instance starts at the default').toBe(`${DEFAULT_WIDGET_HEIGHT_PX}px`);

    act(() => { render(null, host); });
    act(() => {
      render(<WidgetFrame appId="lucidos-sound-player" params={{ clip: 'a.mp3' }} reveal="on-load" place="shelf" />, host);
    });
    expect(frameBox().style.height).toBe('48px');
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
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="embed" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    // Its top is 100px above the line, so the reader reads what follows it.
    layOut(frameBox(), 200, boxHeight);

    report(600);
    expect(scroller.scrollTop, 'grown by 360px above the reader').toBe(660);
    report(80);
    expect(scroller.scrollTop, 'shrunk by 520px above the reader').toBe(140);
    render(null, scroller);
  });

  it('holds what follows an inline card, whose clip box caps the frame', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    const clip = document.createElement('div');
    clip.className = 'widget-card-clip';
    scroller.append(clip);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" />, clip); });
    act(() => { reportOnScreen?.(true); });
    layOut(clip, 200, () => Math.min(boxHeight(), 400));

    report(600);
    expect(scroller.scrollTop, 'the card grew to its 400px cap').toBe(460);
    render(null, clip);
  });

  it('leaves a widget the reader is using where it is', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="embed" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    layOut(frameBox(), 200, boxHeight);
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
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="embed" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    frames()[0].tabIndex = 0;
    frames()[0].focus();
    // Its bottom is 60px above the line, so it is wholly off screen.
    layOut(frameBox(), 0, boxHeight);

    report(600);
    expect(scroller.scrollTop, 'grown by 360px above the reader').toBe(660);
    render(null, scroller);
  });

  it('leaves the reader alone when nothing after the widget is on screen', () => {
    const { scroller, layOut } = transcript(300);
    host.append(scroller);
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="embed" />, scroller); });
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
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="embed" />, scroller); });
    act(() => { reportOnScreen?.(true); });
    layOut(frameBox(), 500, boxHeight);

    report(600);
    expect(scroller.scrollTop).toBe(300);
    render(null, scroller);
  });
});
