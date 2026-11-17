// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { decodePoster, mediaPoster, posterSize, serialPoster } from './app-media-bridge';
import { BASE_PATH } from '../../utils/basePath';

const frame = document.createElement('iframe');
const fetchMock = vi.fn();
let live = 0;
let peak = 0;
let captures = 0;
const nativeCreate = document.createElement.bind(document);

beforeEach(() => {
  live = peak = captures = 0;
  document.body.append(frame);
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockReset().mockResolvedValue(new Response(null, { headers: { ETag: 'v1' } }));
  vi.spyOn(document, 'createElement').mockImplementation(((tag: string) => {
    if (tag === 'video') {
      let src = '';
      const video = {
        videoWidth: 1920, videoHeight: 1080, duration: 12,
        onloadeddata: null as (() => void) | null, onerror: null as (() => void) | null,
        set src(value: string) { src = value; live++; peak = Math.max(peak, live); captures++; },
        removeAttribute() { if (src) live--; src = ''; },
        load() { if (src) queueMicrotask(() => video.onloadeddata?.()); },
      };
      return video;
    }
    if (tag === 'canvas') return { getContext: () => ({ drawImage() {} }), toDataURL: () => 'data:image/jpeg;base64,poster' };
    return nativeCreate(tag);
  }) as typeof document.createElement);
});
afterEach(() => { frame.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const request = (name: string) => ({ url: `${BASE_PATH}/data/artifacts/${name}.mp4`, maxSize: 400 });

it('scales to the box, original dimensions and hard cap', () => {
  expect(posterSize(1920, 1080, 400)).toEqual([400, 225]);
  expect(posterSize(1080, 1920, 3000)).toEqual([720, 1280]);
  expect(posterSize(100, 50, 400)).toEqual([100, 50]);
});

it('checks credential-less reach before decoding or returning a cached poster', async () => {
  const args = request('reach');
  await mediaPoster(args, frame);
  expect(captures).toBe(1);
  fetchMock.mockResolvedValue(new Response(null, { status: 403 }));
  await expect(mediaPoster(args, frame)).rejects.toThrow('cannot read');
  expect(captures).toBe(1);
  expect(fetchMock.mock.calls[0][1]).toMatchObject({ method: 'HEAD', credentials: 'omit', redirect: 'error' });
});

it('reuses one file version and captures a changed validator', async () => {
  const args = request('versions');
  await mediaPoster(args, frame);
  await mediaPoster(args, frame);
  expect(captures).toBe(1);
  fetchMock.mockResolvedValue(new Response(null, { headers: { ETag: 'v2' } }));
  await mediaPoster(args, frame);
  expect(captures).toBe(2);
});

it('never reuses a URL without a validator', async () => {
  fetchMock.mockResolvedValue(new Response());
  await mediaPoster(request('mutable'), frame);
  await mediaPoster(request('mutable'), frame);
  expect(captures).toBe(2);
});

it('serializes decoders and releases each before the next capture', async () => {
  await Promise.all([mediaPoster(request('one'), frame), mediaPoster(request('two'), frame)]);
  expect(peak).toBe(1);
  expect(live).toBe(0);
});

it('releases a failed decoder and lets the queue continue', async () => {
  const create = vi.mocked(document.createElement).getMockImplementation()!;
  const spy = vi.spyOn(document, 'createElement').mockImplementation(((tag: string) => {
    if (tag === 'canvas') throw new Error('capture failed');
    return create.call(document, tag);
  }) as typeof document.createElement);
  await expect(serialPoster(() => decodePoster('https://example.com/broken.mp4', 400))).rejects.toThrow('capture failed');
  expect(live).toBe(0);
  spy.mockImplementation(create);
  await mediaPoster(request('after-failure'), frame);
  expect(live).toBe(0);
});

it('refuses API routes, credentials and detached requesters', async () => {
  for (const url of [`${BASE_PATH}/api/v1/credentials`, 'https://user:pass@example.com/a.mp4', 'http://example.com/a.mp4']) {
    await expect(mediaPoster({ url, maxSize: 400 }, frame)).rejects.toThrow();
  }
  frame.remove();
  await expect(mediaPoster(request('gone'), frame)).rejects.toThrow('closed');
  expect(fetchMock).not.toHaveBeenCalled();
});

it('reads a persisted poster after checking reach without allocating a decoder', async () => {
  const poster = { poster: 'data:image/jpeg;base64,cached', width: 1920, height: 1080, duration: 12 };
  const match = vi.fn().mockResolvedValue(new Response(JSON.stringify(poster)));
  vi.stubGlobal('caches', { open: vi.fn().mockResolvedValue({ match }) });
  expect(await mediaPoster(request('persisted'), frame)).toEqual(poster);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(match).toHaveBeenCalledTimes(1);
  expect(captures).toBe(0);
});
