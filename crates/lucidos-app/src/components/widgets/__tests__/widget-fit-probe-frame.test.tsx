// @vitest-environment jsdom
/**
 * A *widget fit check* frame loads at once, though nothing shows it. It asks
 * the widget to measure for each token, and hears only its own answer.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { WIDGET_CHANNEL } from '@lucidos/widget-frame';

const { postToAppFrame } = vi.hoisted(() => ({ postToAppFrame: vi.fn() }));
vi.mock('../../../store/actions/app-bridge', () => ({ postToAppFrame }));

import { WidgetFrame, _resetWidgetHeightsForTesting, type WidgetFitProbe } from '../WidgetFrame';
import { APP_FRAME_FIT_EVENT, APP_FRAME_SIZE_EVENT, type WidgetFit } from '../../../store/actions/widget-frame-bridge';

let host: HTMLElement;

beforeEach(() => {
  postToAppFrame.mockReset();
  _resetWidgetHeightsForTesting();
  vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => { run(performance.now()); return 0; });
  // Nothing on screen: a probe sits where no reader sees it.
  vi.stubGlobal('IntersectionObserver', class {
    constructor(private readonly report: (entries: Array<{ isIntersecting: boolean }>) => void) {}
    observe() { this.report([{ isIntersecting: false }]); }
    disconnect() {}
  });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  vi.unstubAllGlobals();
  render(null, host);
  host.remove();
});

const frame = () => host.querySelector('iframe');
const lastPush = () => {
  const pushes = postToAppFrame.mock.calls.filter((c) => c[1] === WIDGET_CHANNEL);
  return pushes[pushes.length - 1]?.[2];
};
const answer = (fit: WidgetFit) => act(() => {
  frame()!.dispatchEvent(new CustomEvent(APP_FRAME_FIT_EVENT, { detail: fit, bubbles: true }));
});
const fits = (token: string): WidgetFit => ({ token, box: { width: 320, height: 320 }, findings: [], total: 0 });

function probeFrame(probe: WidgetFitProbe) {
  act(() => { render(<WidgetFrame appId="trip" reveal="on-load" place="dashboard" size="large" probe={probe} />, host); });
}

describe('a widget fit check frame', () => {
  it('loads at once, off screen, on the device it names', () => {
    probeFrame({ measure: 't1', device: 'the-phone', onFit: () => {} });
    expect(frame()).not.toBeNull();
    expect(new URL(frame()!.src).searchParams.get('device')).toBe('the-phone');
  });

  it('asks the widget to measure for each new token', () => {
    const onFit = vi.fn();
    probeFrame({ measure: 't1', device: null, onFit });
    act(() => { frame()!.dispatchEvent(new Event('load')); });
    expect(lastPush()).toMatchObject({ place: 'dashboard', size: 'large', measure: 't1' });

    act(() => {
      render(<WidgetFrame appId="trip" reveal="on-load" place="dashboard" size="xl" probe={{ measure: 't2', device: null, onFit }} />, host);
    });
    expect(lastPush()).toMatchObject({ size: 'xl', measure: 't2' });
  });

  it('hears only the answer to the token it asked last', () => {
    const onFit = vi.fn();
    probeFrame({ measure: 't2', device: null, onFit });
    answer(fits('t1'));
    expect(onFit).not.toHaveBeenCalled();
    answer(fits('t2'));
    expect(onFit).toHaveBeenCalledWith(fits('t2'));
  });

  it('leaves no size behind for a card of the same widget', () => {
    probeFrame({ measure: 't1', device: null, onFit: () => {} });
    act(() => {
      frame()!.dispatchEvent(new CustomEvent(APP_FRAME_SIZE_EVENT, { detail: { height: 900 }, bubbles: true }));
    });
    act(() => { render(null, host); });
    const sizes: number[] = [];
    act(() => {
      render(<WidgetFrame appId="trip" reveal="on-load" place="dashboard" size="large" onSize={(s) => sizes.push(s.height)} />, host);
    });
    expect(sizes[0]).not.toBe(900);
  });
});
