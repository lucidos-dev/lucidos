// @vitest-environment jsdom
/**
 * The frame's wiring to its load probe (appFrameLoadProbe.ts): the probe is
 * told of the load and the ready signal, and it follows the element, so a
 * reload takes a fresh probe. The probe's own rules are in its unit test.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const postClientLog = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/clientLog', () => ({ postClientLog }));

import { AppFrame } from '../AppFrame';
import { SLOW_LOAD_MS } from '../appFrameLoadProbe';
import { APP_FRAME_READY_EVENT } from '../../../store/actions/app-ready-bridge';
import type { AppReveal } from '../../../store/types';

let hosts: HTMLElement[];

function mount(app: string, reveal: AppReveal = 'on-load'): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  hosts.push(host);
  act(() => { render(<AppFrame src={`/app/${app}/`} reveal={reveal} label={{ app, place: 'dashboard' }} />, host); });
  return host;
}

const frameIn = (host: HTMLElement) => host.querySelector('iframe')!;
const fireLoad = (host: HTMLElement) => act(() => { frameIn(host).dispatchEvent(new Event('load')); });
const fireReady = (host: HTMLElement) => act(() => { frameIn(host).dispatchEvent(new Event(APP_FRAME_READY_EVENT)); });
const unmount = (host: HTMLElement) => act(() => { render(null, host); });
const messages = () => postClientLog.mock.calls.map((c) => c[1]);

beforeEach(() => {
  vi.useFakeTimers();
  hosts = [];
  postClientLog.mockClear();
});

afterEach(() => {
  for (const host of hosts) {
    render(null, host);
    host.remove();
  }
  vi.useRealTimers();
});

describe('AppFrame load probe', () => {
  it('stays quiet for a frame that loads in time', () => {
    const host = mount('fast');
    vi.advanceTimersByTime(200);
    fireLoad(host);
    vi.advanceTimersByTime(SLOW_LOAD_MS * 2);
    expect(postClientLog).not.toHaveBeenCalled();
  });

  it('names an on-ready frame that loaded but never called ready, then closes the line', () => {
    const host = mount('late', 'on-ready');
    fireLoad(host);
    act(() => { vi.advanceTimersByTime(SLOW_LOAD_MS); });
    expect(postClientLog).toHaveBeenCalledWith('app-frame', 'slow load', expect.objectContaining({
      app: 'late', place: 'dashboard', loaded: true, ready: false,
    }));

    act(() => { vi.advanceTimersByTime(2000); });
    fireReady(host);
    expect(messages()).toEqual(['slow load', 'slow load ended']);
    expect(postClientLog.mock.calls[1][2]).toMatchObject({ readyMs: SLOW_LOAD_MS + 2000, fused: false });
  });

  it('writes abandoned when a slow frame unmounts', () => {
    const host = mount('gone');
    act(() => { vi.advanceTimersByTime(SLOW_LOAD_MS); });
    unmount(host);
    expect(messages()).toEqual(['slow load', 'slow load abandoned']);
  });

  it('takes a fresh probe when a sibling leaving reloads the frame', () => {
    const leaving = mount('leaving');
    const waiting = mount('waiting');
    act(() => { vi.advanceTimersByTime(SLOW_LOAD_MS - 100); });
    unmount(leaving);
    expect(postClientLog).not.toHaveBeenCalled();

    const fresh = frameIn(waiting);
    act(() => { vi.advanceTimersByTime(SLOW_LOAD_MS); });
    expect(messages()).toEqual(['slow load']);
    expect(postClientLog.mock.calls[0][2]).toMatchObject({ app: 'waiting', sinceMountMs: SLOW_LOAD_MS });
    expect(frameIn(waiting)).toBe(fresh);
  });
});
