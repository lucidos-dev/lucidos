// @vitest-environment jsdom
/**
 * One cap on mounted widget frames across a thread (ADR 0415). Past it, the
 * frame furthest from view unmounts first.
 */
import { describe, it, expect, afterEach, beforeAll, afterAll, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (_el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    onChange(true);
    return () => {};
  },
}));

const { WidgetFrame } = await import('../WidgetFrame');
const { MAX_MOUNTED_WIDGET_FRAMES, mountedWidgetFrameCount } = await import('../widgetFrameBudget');

const host = document.createElement('div');
document.body.append(host);

afterEach(() => {
  act(() => { render(null, host); });
});

// Each frame sits 100px below the last on a 400px viewport, by its index.
const realRect = HTMLElement.prototype.getBoundingClientRect;
beforeAll(() => {
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: 400 });
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const i = Number(this.closest<HTMLElement>('[data-index]')?.dataset.index ?? 0);
    return { top: i * 100, bottom: i * 100 + 90 } as DOMRect;
  };
});
afterAll(() => {
  HTMLElement.prototype.getBoundingClientRect = realRect;
});

describe('the widget frame budget', () => {
  it('mounts at most the cap, and unmounts the frame furthest from view', () => {
    const count = MAX_MOUNTED_WIDGET_FRAMES + 2;
    act(() => {
      render(
        <div>
          {Array.from({ length: count }, (_, i) => (
            <div key={i} data-index={i}>
              <WidgetFrame appId="player" params={{ clip: `${i}.mp3` }} reveal="on-load" place="option" />
            </div>
          ))}
        </div>,
        host,
      );
    });
    expect(host.querySelectorAll('iframe')).toHaveLength(MAX_MOUNTED_WIDGET_FRAMES);
    expect(mountedWidgetFrameCount()).toBe(MAX_MOUNTED_WIDGET_FRAMES);
    const mountedIndexes = Array.from(host.querySelectorAll('iframe'))
      .map((f) => Number(f.closest<HTMLElement>('[data-index]')!.dataset.index));
    expect(mountedIndexes, 'the two furthest from view went').toEqual(
      Array.from({ length: MAX_MOUNTED_WIDGET_FRAMES }, (_, i) => i),
    );

    act(() => { render(null, host); });
    expect(mountedWidgetFrameCount(), 'an unmount releases its claim').toBe(0);
  });

  it('never evicts a frame in view, even past the cap', () => {
    const count = MAX_MOUNTED_WIDGET_FRAMES + 1;
    act(() => {
      render(
        <div>
          {Array.from({ length: count }, (_, i) => (
            // Every frame at index 0 sits in view.
            <div key={i} data-index={0}>
              <WidgetFrame appId="player" params={{ clip: `${i}.mp3` }} reveal="on-load" place="option" />
            </div>
          ))}
        </div>,
        host,
      );
    });
    expect(host.querySelectorAll('iframe')).toHaveLength(count);
  });
});
