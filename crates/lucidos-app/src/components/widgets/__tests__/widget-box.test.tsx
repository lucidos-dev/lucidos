// @vitest-environment jsdom
/**
 * A size change never shows the widget's old layout in the new box (plan
 * 2026-11-09-frameless-centred-inline-widgets, invariants 5 and 6). The box
 * covers itself at once, and only a report naming the new size, or the fuse,
 * clears the cover.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
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

import type { WidgetSize } from '../../../store/types';
import { WidgetBox, RESIZE_COVER_FUSE_MS } from '../WidgetBox';
import { _resetWidgetHeightsForTesting } from '../WidgetFrame';
import { APP_FRAME_SIZE_EVENT } from '../../../store/actions/widget-frame-bridge';

let host: HTMLElement;

beforeEach(() => {
  _resetWidgetHeightsForTesting();
  vi.useFakeTimers();
  vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => { run(performance.now()); return 0; });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const box = (size: WidgetSize) => (
  <WidgetBox appId="fare-grid" reveal="on-load" place="inline" size={size} />
);
const covered = () => host.querySelector('.widget-box-cover')!.classList.contains('is-covering');
const report = (detail: { height: number; width?: number; size?: WidgetSize }) => act(() => {
  host.querySelector('iframe')!.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail, bubbles: true }));
});

function shownAt(size: WidgetSize): void {
  act(() => { render(box(size), host); });
  act(() => { reportOnScreen?.(true); });
}

describe('WidgetBox resize cover', () => {
  it('shows no cover when the box first draws', () => {
    shownAt('medium');
    expect(covered()).toBe(false);
  });

  it('covers the box in the same render that takes the new size', () => {
    shownAt('medium');
    act(() => { render(box('large'), host); });
    expect(host.querySelector('.widget-box')!.getAttribute('data-size')).toBe('large');
    expect(covered()).toBe(true);
  });

  it('keeps the cover through a report for the old size, or one naming no size', () => {
    shownAt('medium');
    act(() => { render(box('large'), host); });
    report({ height: 200, size: 'medium' });
    report({ height: 200 });
    expect(covered()).toBe(true);
  });

  it('clears the cover once the widget reports a layout for the new size', () => {
    shownAt('medium');
    act(() => { render(box('large'), host); });
    report({ height: 400, size: 'large' });
    expect(covered()).toBe(false);
  });

  it('clears the cover when no report comes, once the fuse runs out', () => {
    shownAt('medium');
    act(() => { render(box('large'), host); });
    act(() => { vi.advanceTimersByTime(RESIZE_COVER_FUSE_MS - 1); });
    expect(covered()).toBe(true);
    act(() => { vi.advanceTimersByTime(1); });
    expect(covered()).toBe(false);
  });

  it('waits on the latest size when the size changes twice', () => {
    shownAt('small');
    act(() => { render(box('medium'), host); });
    act(() => { render(box('large'), host); });
    report({ height: 300, size: 'medium' });
    expect(covered()).toBe(true);
    report({ height: 400, size: 'large' });
    expect(covered()).toBe(false);
  });
});

/** Invariant: the right-edge fade shows exactly when the content is wider than
 *  the canvas it draws on, and never on a transparent box (plan
 *  2026-11-10-widget-fit-check). */
describe('WidgetBox right-edge fade', () => {
  const rightFade = () => host.querySelector('.widget-box-fade[data-edge="right"]');
  const canvasWidth = (px: number) => {
    Object.defineProperty(host.querySelector('.widget-box-canvas')!, 'clientWidth', { configurable: true, value: px });
  };

  it('fades the right edge when the content spills sideways', () => {
    shownAt('large');
    canvasWidth(320);
    report({ height: 100, width: 410, size: 'large' });
    expect(rightFade()).not.toBeNull();
  });

  it('shows no right fade for content that fits, or a report with no width', () => {
    shownAt('large');
    canvasWidth(320);
    report({ height: 100, width: 320.6, size: 'large' });
    expect(rightFade()).toBeNull();
    report({ height: 100, size: 'large' });
    expect(rightFade()).toBeNull();
  });

  it('never calls an XL box that pans cut: its canvas keeps the true width', () => {
    shownAt('xl');
    canvasWidth(680);
    report({ height: 100, width: 680, size: 'xl' });
    expect(rightFade()).toBeNull();
  });

  it('never fades a transparent box, on either edge', () => {
    act(() => { render(<WidgetBox appId="fare-grid" reveal="on-load" place="inline" size="large" transparent />, host); });
    act(() => { reportOnScreen?.(true); });
    canvasWidth(320);
    report({ height: 2000, width: 900, size: 'large' });
    expect(host.querySelector('.widget-box-fade')).toBeNull();
  });
});
