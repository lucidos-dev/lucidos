// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
import { watchWidgetResources, widgetResources } from './widgetResources';

it('reports counts only and counts loaded videos independently of dimensions', () => {
  document.body.innerHTML = '<video src="private.mp4"></video><video></video><img src="private.png">';
  Object.defineProperties(document.querySelector('video')!, {
    readyState: { value: 1 }, videoWidth: { value: 1920 }, videoHeight: { value: 1080 },
  });
  Object.defineProperties(document.querySelector('img')!, {
    naturalWidth: { value: 100 }, naturalHeight: { value: 50 },
  });
  expect(widgetResources(document)).toEqual({ videoLoaded: 1, videoPixels: 2073600, images: 1, imagePixels: 5000 });
  document.body.innerHTML = '';
});

it('coalesces changes, reports release, and disconnects cleanly', async () => {
  vi.useFakeTimers();
  document.body.innerHTML = '<video></video>';
  const video = document.querySelector('video')!;
  let readyState = 0;
  Object.defineProperty(video, 'readyState', { get: () => readyState });
  const report = vi.fn();
  const stop = watchWidgetResources(document, report);
  await vi.advanceTimersByTimeAsync(20);
  expect(report.mock.lastCall?.[0].videoLoaded).toBe(0);
  readyState = 1;
  video.dispatchEvent(new Event('loadedmetadata'));
  video.dispatchEvent(new Event('loadeddata'));
  await vi.advanceTimersByTimeAsync(20);
  expect(report).toHaveBeenCalledTimes(2);
  expect(report.mock.lastCall?.[0].videoLoaded).toBe(1);
  readyState = 0;
  video.dispatchEvent(new Event('emptied'));
  await vi.advanceTimersByTimeAsync(20);
  expect(report.mock.lastCall?.[0].videoLoaded).toBe(0);
  stop();
  readyState = 1;
  video.dispatchEvent(new Event('loadedmetadata'));
  await vi.advanceTimersByTimeAsync(20);
  expect(report).toHaveBeenCalledTimes(3);
  document.body.innerHTML = '';
  vi.useRealTimers();
});

it('reports releases even when animation frames never run', async () => {
  vi.useFakeTimers();
  const animation = vi.spyOn(globalThis, 'requestAnimationFrame').mockReturnValue(1);
  document.body.innerHTML = '<video></video>';
  const video = document.querySelector('video')!;
  let readyState = 1;
  Object.defineProperty(video, 'readyState', { get: () => readyState });
  const report = vi.fn();
  const stop = watchWidgetResources(document, report);
  readyState = 0;
  video.dispatchEvent(new Event('emptied'));
  await vi.advanceTimersByTimeAsync(20);
  expect(report.mock.lastCall?.[0].videoLoaded).toBe(0);
  stop();
  animation.mockRestore();
  document.body.innerHTML = '';
  vi.useRealTimers();
});
