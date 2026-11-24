// @vitest-environment jsdom
import { expect, it, vi } from 'vitest';
const callHost = vi.hoisted(() => vi.fn().mockResolvedValue({ poster: 'data:image/jpeg;base64,a', width: 100, height: 50, duration: 2 }));
vi.mock('./_bridge', () => ({ callHost }));
vi.mock('./data', () => ({ data: { url: (path: string) => `/workspace/~cap/pass/data/${path}` } }));
import { media } from './media';
it('uses the frame capability URL and scales the box by DPR', async () => {
  vi.stubGlobal('devicePixelRatio', 2);
  await media.poster('artifacts/demo.mp4', { maxSize: 300 });
  expect(callHost).toHaveBeenLastCalledWith('media.poster', { url: '/workspace/~cap/pass/data/artifacts/demo.mp4', maxSize: 600 }, 60000);
  await media.poster('https://example.com/demo.mp4');
  expect(callHost).toHaveBeenLastCalledWith('media.poster', { url: 'https://example.com/demo.mp4', maxSize: 1280 }, 60000);
  vi.unstubAllGlobals();
});
