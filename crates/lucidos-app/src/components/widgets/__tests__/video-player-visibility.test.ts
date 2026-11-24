// @ts-expect-error: Vitest provides Node APIs without project Node types
import { readFileSync } from 'node:fs';
// @ts-expect-error: jsdom is available at runtime without project declarations
import { JSDOM } from 'jsdom';
import { afterEach, describe, expect, it, vi } from 'vitest';

const html = readFileSync(new URL('../../../../../../system-widgets/lucidos-video-player/index.html', import.meta.url), 'utf8');
let dom: { window: Window & typeof globalThis };
afterEach(() => { dom?.window.close(); vi.useRealTimers(); });

function setup(readyState = 2, clip = 'artifacts/sample.mp4', startOnScreen = true, intent = true, posterFails = false) {
  dom = new JSDOM(html, { runScripts: 'outside-only' });
  const { window } = dom;
  const video = window.document.querySelector('video')!;
  let intersect!: (entries: { isIntersecting: boolean }[]) => void;
  let visibility!: (visible: boolean) => void;
  Object.defineProperties(video, {
    readyState: { get: () => readyState, configurable: true },
    load: { value: () => { readyState = 0; } },
    duration: { get: () => 120 },
    paused: { get: () => true },
    seekable: { value: { length: 1, start: () => 0, end: () => 120 }, configurable: true },
    currentTime: { value: 61, writable: true, configurable: true },
  });
  const play = vi.spyOn(video, 'play').mockResolvedValue();
  const poster = vi.fn().mockImplementation(() => posterFails
    ? Promise.reject(new Error('CORS refused'))
    : Promise.resolve({ poster: 'data:image/jpeg;base64,cG9zdGVy', width: 1080, height: 1920, duration: 120 }));
  const toast = vi.fn();
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
      media: { poster },
      ui: { applyPreferences() {}, watchPreferences() {}, toast },
      widget: { onVisibility(callback: typeof visibility) { visibility = callback; } },
    },
  });
  const script = [...window.document.scripts].find(s => s.textContent?.includes('const video'))!;
  window.eval(script.textContent!);
  if (startOnScreen) intersect([{ isIntersecting: true }]);
  if (intent) window.document.querySelector<HTMLButtonElement>('#play')!.click();
  if (intent) video.dispatchEvent(new window.Event('loadedmetadata'));
  return { window, video, pause, play, poster, toast, displays, intersect: (visible: boolean) => intersect([{ isIntersecting: visible }]), visibility: (visible: boolean) => visibility(visible) };
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
    Object.defineProperty(s.video, 'readyState', { get: () => 1 });
    let position = 0;
    Object.defineProperty(s.video, 'currentTime', {
      get: () => position,
      set: (target: number) => { if (s.video.seekable.length) position = target; },
    });
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    const scrub = s.window.document.querySelector<HTMLInputElement>('#scrub')!;
    scrub.value = '10';
    scrub.dispatchEvent(new s.window.Event('input'));
    Object.defineProperty(s.video, 'seekable', { value: { length: 1, start: () => 0, end: () => 120 } });
    s.video.dispatchEvent(new s.window.Event('canplay'));
    expect(s.video.currentTime).toBe(10);
  });

  it('never loads a clip that starts offscreen until it comes on screen', () => {
    const s = setup(0, 'artifacts/sample.mp4', false, false);
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(false);
    s.visibility(true);
    expect(s.video.hasAttribute('src')).toBe(false);
    s.intersect(true);
    s.window.document.querySelector<HTMLButtonElement>('#play')!.click();
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


describe('built-in video player intent', () => {
  it('draws a bounded poster and duration while visible idle media stays unloaded', async () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    await Promise.resolve();
    expect(s.poster).toHaveBeenCalledWith('artifacts/sample.mp4', { maxSize: expect.any(Number) });
    expect(s.video.hasAttribute('src')).toBe(false);
    expect(s.video.readyState).toBe(0);
    expect(s.window.document.querySelector('img')!.src).toContain('data:image/jpeg');
    expect(s.window.document.querySelector('#player')!.getAttribute('style')).toContain('1080 / 1920');
    expect(s.window.document.querySelector('#time')!.textContent).toBe('0:00 / 2:00');
    s.intersect(false);
    s.intersect(true);
    s.visibility(false);
    s.visibility(true);
    s.window.dispatchEvent(new s.window.Event('pageshow'));
    expect(s.video.hasAttribute('src')).toBe(false);
    expect(s.play).not.toHaveBeenCalled();
    expect(s.poster).toHaveBeenCalledOnce();
  });

  it('keeps play usable when a poster is refused', async () => {
    const s = setup(0, 'https://example.com/clip.mp4', true, false, true);
    await Promise.resolve();
    await Promise.resolve();
    expect(s.video.hasAttribute('src')).toBe(false);
    expect(s.window.document.querySelector('.big-play')).not.toBeNull();
    expect(s.window.document.querySelector('.error')).toBeNull();
    s.window.document.querySelector<HTMLButtonElement>('#play')!.click();
    expect(s.video.src).toBe('https://example.com/clip.mp4#t=0.001');
    expect(s.play).toHaveBeenCalledOnce();
  });

  it('loads on scrub and seeks when metadata becomes seekable', () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    const scrub = s.window.document.querySelector<HTMLInputElement>('#scrub')!;
    scrub.max = '120';
    scrub.value = '35';
    scrub.dispatchEvent(new s.window.Event('input'));
    expect(s.video.hasAttribute('src')).toBe(true);
    s.video.currentTime = 0;
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    expect(s.video.currentTime).toBe(35);
    expect(s.play).not.toHaveBeenCalled();
  });

  it('scrubs loaded media without freezing the display on an unseekable target', () => {
    const s = setup();
    s.video.dispatchEvent(new s.window.Event('canplay'));
    Object.defineProperty(s.video, 'readyState', { get: () => 2 });
    Object.defineProperty(s.video, 'seekable', { value: { length: 0 } });
    const scrub = s.window.document.querySelector<HTMLInputElement>('#scrub')!;
    scrub.max = '120';
    scrub.value = '90';
    scrub.dispatchEvent(new s.window.Event('input'));
    expect(s.video.currentTime).toBe(90);
    s.video.currentTime = 91;
    s.video.dispatchEvent(new s.window.Event('timeupdate'));
    expect(scrub.value).toBe('91');
  });

  it('requests standard full screen during the idle tap when supported', () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    const standard = vi.fn().mockResolvedValue(undefined);
    const native = vi.fn();
    Object.defineProperty(s.window.document, 'fullscreenEnabled', { value: true });
    Object.defineProperty(s.video, 'requestFullscreen', { value: standard });
    Object.defineProperty(s.video, 'webkitEnterFullscreen', { value: native });
    s.window.document.querySelector<HTMLButtonElement>('#full')!.click();
    expect(standard).toHaveBeenCalledOnce();
    expect(native).not.toHaveBeenCalled();
    expect(s.play).not.toHaveBeenCalled();
    s.window.document.dispatchEvent(new s.window.Event('fullscreenchange'));
  });

  it('waits for metadata for Safari full screen and falls back to native playback', () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    const full = vi.fn(() => { throw new Error('gesture required'); });
    Object.defineProperty(s.video, 'webkitEnterFullscreen', { value: full });
    s.window.document.querySelector<HTMLButtonElement>('#full')!.click();
    expect(s.video.hasAttribute('src')).toBe(true);
    expect(full).not.toHaveBeenCalled();
    expect(s.play).toHaveBeenCalledOnce();
    expect(s.video.hasAttribute('playsinline')).toBe(false);
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    expect(full).toHaveBeenCalledOnce();
    expect(s.video.hasAttribute('playsinline')).toBe(false);
    expect(s.play).toHaveBeenCalledOnce();
    s.video.dispatchEvent(new s.window.Event('webkitbeginfullscreen'));
    s.video.dispatchEvent(new s.window.Event('webkitendfullscreen'));
    expect(s.video.hasAttribute('playsinline')).toBe(true);
  });

  it('restores inline playback and reports a refused full screen fallback', async () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    Object.defineProperty(s.video, 'webkitEnterFullscreen', { value: () => { throw new Error('gesture'); } });
    s.play.mockRejectedValueOnce(new Error('play refused'));
    s.window.document.querySelector<HTMLButtonElement>('#full')!.click();
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    await Promise.resolve();
    expect(s.toast).toHaveBeenCalledWith(expect.stringContaining('play refused'), 'error');
    expect(s.video.hasAttribute('playsinline')).toBe(true);
  });

  it('reports a stalled full screen request and removes its metadata listener', () => {
    vi.useFakeTimers();
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    const full = vi.fn();
    Object.defineProperty(s.video, 'webkitEnterFullscreen', { value: full });
    s.window.document.querySelector<HTMLButtonElement>('#full')!.click();
    s.intersect(false);
    expect(s.video.hasAttribute('src')).toBe(true);
    vi.advanceTimersByTime(2001);
    expect(s.toast).toHaveBeenCalledOnce();
    expect(s.video.hasAttribute('src')).toBe(false);
    s.video.dispatchEvent(new s.window.Event('loadedmetadata'));
    expect(full).not.toHaveBeenCalled();
  });

  it('keeps the poster until a media frame arrives and brings it back on release', async () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    await Promise.resolve();
    const player = s.window.document.querySelector('#player')!;
    s.window.document.querySelector<HTMLButtonElement>('#play')!.click();
    expect(player.classList.contains('has-frame')).toBe(false);
    s.video.dispatchEvent(new s.window.Event('loadeddata'));
    expect(player.classList.contains('has-frame')).toBe(true);
    s.intersect(false);
    expect(player.classList.contains('has-frame')).toBe(false);
  });

  it('reports a play rejection after intent' , async () => {
    const s = setup(0, 'artifacts/sample.mp4', true, false);
    s.play.mockRejectedValueOnce(new Error('refused'));
    s.window.document.querySelector<HTMLButtonElement>('#play')!.click();
    await Promise.resolve();
    expect(s.window.document.querySelector('.error')!.textContent).toContain('refused');
  });
});
