// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const tellHost = vi.hoisted(() => vi.fn());
vi.mock('./_bridge', () => ({ tellHost, onHostPush: () => () => {} }));

import { FULL_SCREEN_WAIT_MS, installFullScreenTrace } from './fullScreenTrace';

type Proto = Record<string, unknown>;
const videoProto = HTMLVideoElement.prototype as unknown as Proto;
const elementProto = Element.prototype as unknown as Proto;

/** The steps the host was told about, in order. */
const steps = () => tellHost.mock.calls.map(([, args]) => (args as { step: string }).step);
const sent = (step: string) => tellHost.mock.calls
  .map(([, args]) => args as Record<string, unknown>)
  .filter((args) => args.step === step);

let uninstall: () => void;
let enter: ReturnType<typeof vi.fn>;
let video: HTMLVideoElement;

beforeEach(() => {
  vi.useFakeTimers();
  enter = vi.fn(() => 'entered');
  videoProto.webkitEnterFullscreen = enter;
  elementProto.requestFullscreen = vi.fn(() => Promise.resolve());
  uninstall = installFullScreenTrace();
  video = document.createElement('video');
  document.body.append(video);
});

afterEach(() => {
  uninstall();
  delete videoProto.webkitEnterFullscreen;
  delete elementProto.requestFullscreen;
  video.remove();
  tellHost.mockReset();
  vi.useRealTimers();
});

describe('installFullScreenTrace', () => {
  it('runs the call as the app made it: this, arguments and return value', () => {
    const result = (video as unknown as { webkitEnterFullscreen: (...a: unknown[]) => unknown }).webkitEnterFullscreen(1, 'two');

    expect(result).toBe('entered');
    expect(enter).toHaveBeenCalledWith(1, 'two');
    expect(enter.mock.contexts[0]).toBe(video);
  });

  it('returns the very promise the browser gave, so its rejection stays the app to handle', () => {
    const promise = Promise.resolve();
    uninstall();
    elementProto.requestFullscreen = vi.fn(() => promise);
    uninstall = installFullScreenTrace();

    expect(video.requestFullscreen()).toBe(promise);
  });

  it('rethrows the error the browser threw, and logs it once, with no never-opened to follow', () => {
    const failure = new Error('InvalidStateError');
    enter.mockImplementation(() => { throw failure; });

    expect(() => (video as unknown as { webkitEnterFullscreen: () => void }).webkitEnterFullscreen()).toThrow(failure);
    vi.advanceTimersByTime(FULL_SCREEN_WAIT_MS * 2);

    expect(steps()).toEqual(['request', 'threw']);
  });

  it('never throws into the app when the host is gone', () => {
    tellHost.mockImplementation(() => { throw new Error('no host'); });

    expect(() => (video as unknown as { webkitEnterFullscreen: () => void }).webkitEnterFullscreen()).not.toThrow();
    expect(enter).toHaveBeenCalledTimes(1);
  });

  it('logs the video state at the request', () => {
    Object.defineProperty(video, 'readyState', { value: 4 });
    Object.defineProperty(video, 'webkitSupportsFullscreen', { value: true });
    (video as unknown as { webkitEnterFullscreen: () => void }).webkitEnterFullscreen();

    expect(sent('request')).toEqual([{
      step: 'request',
      api: 'webkitEnterFullscreen',
      element: 'video',
      gesture: undefined,
      readyState: 4,
      srcSet: false,
      supportsFullscreen: true,
    }]);
  });

  it('logs one never-opened line for a request that gets no begin event', () => {
    (video as unknown as { webkitEnterFullscreen: () => void }).webkitEnterFullscreen();
    vi.advanceTimersByTime(FULL_SCREEN_WAIT_MS * 3);

    expect(steps()).toEqual(['request', 'never-opened']);
    expect(sent('never-opened')[0]).toMatchObject({ element: 'video', api: 'webkitEnterFullscreen' });
  });

  it('logs opened and no never-opened when the player begins', () => {
    (video as unknown as { webkitEnterFullscreen: () => void }).webkitEnterFullscreen();
    video.dispatchEvent(new Event('webkitbeginfullscreen'));
    vi.advanceTimersByTime(FULL_SCREEN_WAIT_MS * 2);

    expect(steps()).toEqual(['request', 'opened']);
  });

  it('logs closed when the player ends', () => {
    video.dispatchEvent(new Event('webkitendfullscreen'));

    expect(steps()).toEqual(['closed']);
  });

  it('reads a fullscreenchange as opened or closed by the document fullscreen element', () => {
    video.requestFullscreen();
    Object.defineProperty(document, 'fullscreenElement', { value: video, configurable: true });
    video.dispatchEvent(new Event('fullscreenchange', { bubbles: true }));
    Object.defineProperty(document, 'fullscreenElement', { value: null, configurable: true });
    video.dispatchEvent(new Event('fullscreenchange', { bubbles: true }));
    vi.advanceTimersByTime(FULL_SCREEN_WAIT_MS * 2);

    expect(steps()).toEqual(['request', 'opened', 'closed']);
  });

  it('restores the browser methods and stops listening on teardown', () => {
    uninstall();

    expect(videoProto.webkitEnterFullscreen).toBe(enter);
    video.dispatchEvent(new Event('webkitbeginfullscreen'));
    expect(tellHost).not.toHaveBeenCalled();
    uninstall = installFullScreenTrace();
  });

  it('skips a method the browser does not have', () => {
    uninstall();
    delete videoProto.webkitEnterFullscreen;

    expect(() => { uninstall = installFullScreenTrace(); }).not.toThrow();
    expect(videoProto.webkitEnterFullscreen).toBeUndefined();
  });
});
