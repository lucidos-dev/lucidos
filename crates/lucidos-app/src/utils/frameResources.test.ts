// @vitest-environment jsdom
import { expect, it } from 'vitest';
import { frameResources, noteFrameResources } from './frameResources';
import { pageResources } from './pageResources';

it('sums mounted frames, clamps counts and keeps only numeric fields', () => {
  document.body.innerHTML = '<iframe></iframe><iframe></iframe>';
  const [a, b] = document.querySelectorAll('iframe');
  noteFrameResources(a, { videoLoaded: 2, videoPixels: 400, images: 1, imagePixels: 100, path: 'secret' });
  noteFrameResources(b, { videoLoaded: 20000, videoPixels: 2e9, images: 0, imagePixels: 0 });
  expect(frameResources(document)).toEqual({ frameVideoLoaded: 10002, frameVideoPixels: 1000000400, frameImagePixels: 100 });
  a.remove();
  expect(frameResources(document).frameVideoLoaded).toBe(10000);
  document.body.innerHTML = '';
  expect(frameResources(document).frameVideoLoaded).toBe(0);
});

it('rejects malformed counts as a whole', () => {
  document.body.innerHTML = '<iframe></iframe>';
  const frame = document.querySelector('iframe')!;
  for (const videoPixels of [NaN, Infinity, -1, '100', 1.5]) {
    noteFrameResources(frame, { videoLoaded: 1, videoPixels, images: 1, imagePixels: 10 });
    expect(frameResources(document).frameVideoLoaded).toBe(0);
  }
  document.body.innerHTML = '';
});

it('deduplicates host image pixels without exposing the URL', () => {
  document.body.innerHTML = '<img src="private.png"><img src="private.png">';
  for (const img of document.images) Object.defineProperties(img, {
    naturalWidth: { value: 1920 }, naturalHeight: { value: 1080 },
  });
  const unloaded = document.createElement('img');
  unloaded.src = 'private.png';
  document.body.append(unloaded);
  const sample = pageResources(null);
  expect(sample.imagePixels).toBe(4147200);
  expect(sample.uniqueImagePixels).toBe(2073600);
  expect(JSON.stringify(sample)).not.toContain('private');
  document.body.innerHTML = '';
});
