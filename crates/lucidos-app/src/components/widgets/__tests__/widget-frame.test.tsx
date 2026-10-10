// @vitest-environment jsdom
/**
 * A widget's frame IS an app frame (ADR 0402, plan invariant 1). An inline one
 * mounts only while on screen (invariant 3); a shelf one mounts at once.
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

import { WidgetFrame, DEFAULT_WIDGET_HEIGHT_PX, _resetWidgetHeightsForTesting } from '../WidgetFrame';
import { APP_FRAME_SANDBOX, APP_FRAME_ALLOW } from '../../apps/appFrameSandbox';
import { APP_FRAME_HEIGHT_EVENT } from '../../../store/actions/app-height-bridge';
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

  it('holds no inline iframe off screen, and one on screen', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="inline" />, host); });
    expect(frames()).toHaveLength(0);
    expect(frameBox().classList.contains('widget-frame-inline')).toBe(true);
    expect(frameBox().style.height).toBe(`${DEFAULT_WIDGET_HEIGHT_PX}px`);

    act(() => { reportOnScreen?.(true); });
    expect(frames()).toHaveLength(1);

    act(() => { reportOnScreen?.(false); });
    expect(frames()).toHaveLength(0);
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
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_HEIGHT_EVENT, { detail: 312, bubbles: true }));
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
      frames()[0].dispatchEvent(new CustomEvent(APP_FRAME_HEIGHT_EVENT, { detail: 48, bubbles: true }));
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
