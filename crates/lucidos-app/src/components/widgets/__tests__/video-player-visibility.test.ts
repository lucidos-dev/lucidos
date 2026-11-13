// @ts-expect-error: Vitest provides Node APIs without project Node types
import { readFileSync } from 'node:fs';
// @ts-expect-error: jsdom is available at runtime without project declarations
import { JSDOM } from 'jsdom';
import { afterEach, describe, expect, it, vi } from 'vitest';

const html = readFileSync(new URL('../../../../../../system-widgets/lucidos-video-player/index.html', import.meta.url), 'utf8');
let dom: { window: Window & typeof globalThis };
afterEach(() => dom?.window.close());

function setup(readyState = 2, clip = 'artifacts/sample.mp4', startOnScreen = true) {
  dom = new JSDOM(html, { runScripts: 'outside-only' });
  const { window } = dom;
  const video = window.document.querySelector('video')!;
  let intersect!: (entries: { isIntersecting: boolean }[]) => void;
  let visibility!: (visible: boolean) => void;
  Object.defineProperties(video, {
    readyState: { get: () => readyState },
    load: { value: () => { readyState = 0; } },
    duration: { get: () => 120 },
    paused: { get: () => true },
    seekable: { value: { length: 1, start: () => 0, end: () => 120 }, configurable: true },
    currentTime: { value: 61, writable: true },
  });
  const pause = vi.spyOn(video, 'pause').mockImplementation(() => {});
  const displays: string[] = [];
  Object.defineProperty(video, 'offsetHeight', {
    get: () => { displays.push(video.style.display); return 100; },
  });
  Object.assign(window, {
    IntersectionObserver: class {
      constructor(callback: typeof intersect) { intersect = callback; }
      observe(target: Element) { expect(target).toBe(video); }
    },
    lucidos: {
      params: () => ({ clip }),
      data: { url: (path: string) => '/data/' + path },
      ui: { applyPreferences() {}, watchPreferences() {} },
      widget: { onVisibility(callback: typeof visibility) { visibility = callback; } },
    },
  });
  const script = [...window.document.scripts].find(s => s.textContent?.includes('const video'))!;
  window.eval(script.textContent!);
  if (startOnScreen) intersect([{ isIntersecting: true }]);
  video.dispatchEvent(new window.Event('loadedmetadata'));
  return { window, video, pause, displays, intersect: (visible: boolean) => intersect([{ isIntersecting: visible }]), visibility: (visible: boolean) => visibility(visible) };
}

describe('built-in video player visibility', () => {
  it('lets go of its media offscreen and reloads it at the same position on return', () => {
    const s = setup();
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=0.001');
    s.intersect(false);
    expect(s.pause).toHaveBeenCalledOnce();
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=61');
    expect(s.displays).toEqual([]);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=61');
  });

  it('keeps its position when it leaves again before the reload has metadata', () => {
    const s = setup();
    s.intersect(true);
    s.intersect(false);
    s.video.currentTime = 0;
    s.intersect(true);
    s.intersect(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=61');
  });

  it('seeks a clip with its own fragment back to its position at the metadata', () => {
    const s = setup(2, 'artifacts/sample.mp4#xywh=0,0,10,10');
    s.intersect(true);
    s.intersect(false);
    s.video.currentTime = 0;
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#xywh=0,0,10,10');
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    expect(s.video.currentTime).toBe(61);
  });

  it('keeps the position of a clip with its own fragment until it can seek there', () => {
    const s = setup(2, 'artifacts/sample.mp4#xywh=0,0,10,10');
    s.intersect(true);
    s.intersect(false);
    s.video.currentTime = 0;
    s.intersect(true);
    const ranges = { length: 1, start: () => 0, end: () => 120 };
    Object.defineProperty(s.video, 'seekable', { value: { length: 0 }, configurable: true });
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    expect(s.video.currentTime).toBe(0);
    Object.defineProperty(s.video, 'seekable', { value: ranges });
    s.video.dispatchEvent(new s.window.Event('canplay'));
    expect(s.video.currentTime).toBe(61);
  });

  it.each([
    ['artifacts/sample.mp4#t=30', '/data/artifacts/sample.mp4#t=30'],
    ['artifacts/sample.mp4', '/data/artifacts/sample.mp4#t=0.001'],
  ])('keeps the start of %s when it leaves before its metadata', (clip, src) => {
    const s = setup(0, clip);
    s.intersect(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe(src);
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    s.video.dispatchEvent(new s.window.Event('canplay'));
    expect(s.video.currentTime).toBe(61);
  });

  it('lets a scrub before the clip can seek win over its old position', () => {
    const s = setup(2, 'artifacts/sample.mp4#xywh=0,0,10,10');
    s.intersect(true);
    s.intersect(false);
    s.intersect(true);
    Object.defineProperty(s.video, 'seekable', { value: { length: 0 }, configurable: true });
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    const scrub = s.window.document.querySelector<HTMLInputElement>('#scrub')!;
    scrub.value = '10';
    scrub.dispatchEvent(new s.window.Event('input'));
    Object.defineProperty(s.video, 'seekable', { value: { length: 1, start: () => 0, end: () => 120 } });
    s.video.dispatchEvent(new s.window.Event('canplay'));
    expect(s.video.currentTime).toBe(10);
  });

  it('never loads a clip that starts offscreen until it comes on screen', () => {
    const s = setup(2, 'artifacts/sample.mp4', false);
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(false);
    s.visibility(true);
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=0.001');
  });

  it('lets go while the host hides it and reloads once it is shown and on screen', () => {
    const s = setup();
    s.visibility(false);
    expect(s.pause).toHaveBeenCalledOnce();
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(false);
    s.visibility(true);
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=61');
  });

  it('ignores the error event of a released clip', () => {
    const s = setup();
    s.intersect(false);
    s.video.dispatchEvent(new s.window.Event('error'));
    expect(s.window.document.querySelector('.error')).toBeNull();
  });

  it('repaints on document visibility and page restoration', () => {
    const s = setup();
    Object.defineProperty(s.window.document, 'visibilityState', { value: 'visible', configurable: true });
    s.window.document.dispatchEvent(new s.window.Event('visibilitychange'));
    s.window.dispatchEvent(new s.window.Event('pageshow'));
    expect(s.displays).toEqual(['none', 'none']);
    expect(s.video.style.display).toBe('');
    expect(s.video.currentTime).toBeCloseTo(61.002);
    const scrub = s.window.document.querySelector<HTMLInputElement>('#scrub')!;
    expect(Number(scrub.value)).toBeCloseTo(61.002);
    expect(s.window.document.querySelector('#time')!.textContent).toBe('1:01 / 2:00');
    Object.defineProperty(s.window.document, 'visibilityState', { value: 'hidden' });
    s.window.document.dispatchEvent(new s.window.Event('visibilitychange'));
    expect(s.displays).toHaveLength(2);
  });

  it.each(['fullscreen', 'picture-in-picture'])('leaves native %s playback alone', mode => {
    const s = setup();
    Object.defineProperty(s.video, 'webkitPresentationMode', { value: mode });
    s.intersect(false);
    s.visibility(false);
    s.intersect(true);
    s.visibility(true);
    s.window.dispatchEvent(new s.window.Event('pageshow'));
    expect(s.pause).not.toHaveBeenCalled();
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=0.001');
    expect(s.displays).toHaveLength(0);
  });

  it.each([
    ['webkitpresentationmodechanged', 'video'],
    ['webkitendfullscreen', 'video'],
    ['leavepictureinpicture', 'video'],
    ['fullscreenchange', 'document'],
  ])('lets go once native playback ends out of sight (%s)', (event, target) => {
    const s = setup();
    Object.defineProperty(s.video, 'webkitPresentationMode', { value: 'picture-in-picture', configurable: true });
    s.intersect(false);
    expect(s.video.hasAttribute('src')).toBe(true);
    Object.defineProperty(s.video, 'webkitPresentationMode', { value: 'inline' });
    (target === 'video' ? s.video : s.window.document).dispatchEvent(new s.window.Event(event));
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(true);
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=61');
  });

  it('keeps the media when native playback ends on screen', () => {
    const s = setup();
    s.video.dispatchEvent(new s.window.Event('webkitpresentationmodechanged'));
    expect(s.video.getAttribute('src')).toBe('/data/artifacts/sample.mp4#t=0.001');
  });

  it('repaints unseekable media without rewinding it', () => {
    const s = setup();
    Object.defineProperty(s.video, 'seekable', { value: { length: 0 } });
    s.window.dispatchEvent(new s.window.Event('pageshow'));
    expect(s.displays).toEqual(['none']);
    expect(s.video.currentTime).toBe(61);
  });

  it('repaints without seeking before metadata is ready', () => {
    const s = setup(0);
    s.window.dispatchEvent(new s.window.Event('pageshow'));
    expect(s.displays).toEqual(['none']);
    expect(s.video.currentTime).toBe(61);
  });
});
