// @vitest-environment jsdom
/**
 * A window `focus` that hands focus back from one of the page's own frames is
 * no resume. Each case puts focus in a frame, then taps an answer on the host.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';

vi.mock('./platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./platform')>()),
  isWebKit: () => true,
  isIOS: () => true,
}));

import { onPageResume } from './pageResume';

let activeElement: Element | null = null;
let option: HTMLButtonElement;
let answered: Mock<() => void>;
let repaint: Mock<() => void>;
let stop: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  activeElement = document.body;
  Object.defineProperty(document, 'activeElement', { configurable: true, get: () => activeElement });
  document.body.innerHTML = '<iframe></iframe><button class="question-option">Find new tracks</button>';
  option = document.querySelector('.question-option')!;
  answered = vi.fn<() => void>();
  option.addEventListener('click', answered);
  repaint = vi.fn<() => void>();
  stop = onPageResume(repaint);
});

afterEach(() => {
  stop();
  vi.useRealTimers();
  document.body.innerHTML = '';
});

/** The press that plays the clip: focus leaves the host for the frame. */
function focusTheFrame(): void {
  window.dispatchEvent(new Event('blur'));
  activeElement = document.querySelector('iframe');
  vi.advanceTimersByTime(1);
}

function hideThePage(): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
  document.dispatchEvent(new Event('visibilitychange'));
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
}

/** A tap on the answer that also hands focus back to the host window. */
function tapTheAnswer(): void {
  activeElement = document.body;
  window.dispatchEvent(new Event('focus'));
  option.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
}

describe('a frame handing focus back', () => {
  it('lets the answer tap through', () => {
    focusTheFrame();
    tapTheAnswer();
    expect(answered).toHaveBeenCalledTimes(1);
  });

  it('still repaints, once, since a tray restore can hand focus back unseen', () => {
    // Once: an armed guard would repaint a second time on the tap.
    focusTheFrame();
    tapTheAnswer();
    expect(repaint).toHaveBeenCalledTimes(1);
  });

  it('still counts as a resume once the page was hidden in between', () => {
    // Backgrounded with focus in the frame: the layer may be black on return.
    focusTheFrame();
    hideThePage();
    tapTheAnswer();
    expect(answered).not.toHaveBeenCalled();
  });

  it('still counts as a resume when the hide lands before the blur is checked', () => {
    // The press into the frame, then a swipe home before the next tick.
    window.dispatchEvent(new Event('blur'));
    activeElement = document.querySelector('iframe');
    hideThePage();
    vi.advanceTimersByTime(1);
    tapTheAnswer();
    expect(answered).not.toHaveBeenCalled();
  });
});

describe('a window focus with no frame behind it', () => {
  it('still swallows the wake-tap on an answer', () => {
    window.dispatchEvent(new Event('blur'));
    vi.advanceTimersByTime(1);
    tapTheAnswer();
    expect(answered).not.toHaveBeenCalled();
  });
});
