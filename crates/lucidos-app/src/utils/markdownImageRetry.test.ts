// @vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { installMarkdownImageRetry, isImageRetryPending } from './markdownImageRetry';
import { MAX_ATTEMPTS, retryDelayMs } from './imageRetry';

const SRC = '/dev/data/artifacts/shot.png';

let stop: () => void;

function mountImage(src: string, wrapperClass = 'markdown-content'): HTMLImageElement {
  document.body.innerHTML = `<div class="${wrapperClass}"><span class="image-scroll-wrapper"></span></div>`;
  const img = document.createElement('img');
  img.setAttribute('src', src);
  document.querySelector('.image-scroll-wrapper')!.append(img);
  return img;
}

function fail(img: HTMLImageElement): void {
  img.dispatchEvent(new Event('error'));
}

function succeed(img: HTMLImageElement): void {
  img.dispatchEvent(new Event('load'));
}

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = '';
  stop = installMarkdownImageRetry();
});

afterEach(() => {
  stop();
  vi.useRealTimers();
});

describe('installMarkdownImageRetry', () => {
  it('re-requests a failed markdown image with a cache-busted URL', () => {
    const img = mountImage(SRC);
    fail(img);
    expect(img.getAttribute('src'), 'the retry is scheduled, not immediate').toBe(SRC);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src')).toBe(`${SRC}?retry=1`);
  });

  it('busts the ORIGINAL url each time, so retry params never stack', () => {
    const img = mountImage(SRC);
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(1));
    expect(img.getAttribute('src')).toBe(`${SRC}?retry=2`);
  });

  it('gives up after MAX_ATTEMPTS so a genuinely-missing image cannot loop', () => {
    const img = mountImage(SRC);
    for (let i = 0; i < MAX_ATTEMPTS + 3; i++) {
      fail(img);
      vi.advanceTimersByTime(retryDelayMs(i));
    }
    expect(img.getAttribute('src')).toBe(`${SRC}?retry=${MAX_ATTEMPTS}`);
  });

  it('a success cancels the pending retry, and costs the budget nothing', () => {
    const img = mountImage(SRC);
    fail(img);
    succeed(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src'), 'the settled image is left alone').toBe(SRC);
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src'), 'the cancelled attempt was not spent').toBe(`${SRC}?retry=1`);
  });

  it('busts the clean base after a heal, so a later failure cannot stack', () => {
    const img = mountImage(SRC);
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    succeed(img);
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(1));
    expect(img.getAttribute('src')).toBe(`${SRC}?retry=2`);
  });

  it('leaves an image outside a markdown surface alone', () => {
    const img = mountImage(SRC, 'user-image-strip');
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src')).toBe(SRC);
  });

  it('never retries a data: url, whose bytes cannot recover over the network', () => {
    const src = 'data:image/png;base64,AAAA';
    const img = mountImage(src);
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src')).toBe(src);
  });

  it('does not re-request an image a re-render already removed', () => {
    const img = mountImage(SRC);
    fail(img);
    img.remove();
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src')).toBe(SRC);
  });

  it('stops listening once uninstalled', () => {
    const img = mountImage(SRC);
    stop();
    fail(img);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(img.getAttribute('src')).toBe(SRC);
    stop = installMarkdownImageRetry();
  });

  it('says a retry is pending from the failure until the re-request goes out', () => {
    const img = mountImage(SRC);
    expect(isImageRetryPending(img)).toBe(false);
    fail(img);
    expect(isImageRetryPending(img)).toBe(true);
    vi.advanceTimersByTime(retryDelayMs(0));
    expect(isImageRetryPending(img)).toBe(false);
  });
});
