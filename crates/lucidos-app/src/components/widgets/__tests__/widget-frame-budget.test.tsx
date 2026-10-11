// @vitest-environment jsdom
/**
 * One cap on mounted widget frames across a thread (ADR 0415). Past it, the
 * frame furthest from view unmounts first.
 */
import { describe, it, expect, afterEach, beforeAll, afterAll, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const reports = new Map<HTMLElement, (onScreen: boolean) => void>();
vi.mock('../../../hooks/useOnScreenInTranscript', () => ({
  watchOnScreen: (el: HTMLElement, onChange: (onScreen: boolean) => void) => {
    reports.set(el, onChange);
    onChange(true);
    return () => { reports.delete(el); };
  },
}));

const { WidgetFrame } = await import('../WidgetFrame');
const { MAX_MOUNTED_WIDGET_FRAMES, mountedWidgetFrameCount } = await import('../widgetFrameBudget');

const host = document.createElement('div');
document.body.append(host);

afterEach(() => {
  act(() => { render(null, host); });
  scrolledIntoView.clear();
});

// Each frame sits 100px below the last on a 400px viewport, by its index. An
// index in `scrolledIntoView` sits at the top instead.
const scrolledIntoView = new Set<number>();
const realRect = HTMLElement.prototype.getBoundingClientRect;
beforeAll(() => {
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: 400 });
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const i = Number(this.closest<HTMLElement>('[data-index]')?.dataset.index ?? 0);
    const top = scrolledIntoView.has(i) ? 0 : i * 100;
    return { top, bottom: top + 90 } as DOMRect;
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

  it('keeps an evicted frame unloaded until it comes back on screen', () => {
    const count = MAX_MOUNTED_WIDGET_FRAMES + 1;
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
    const last = host.querySelector<HTMLElement>(`[data-index="${count - 1}"] .widget-frame`)!;
    expect(last.querySelector('iframe'), 'evicted, furthest from view').toBeNull();

    // Off screen still: the claim is not taken again, so the cap holds.
    act(() => { reports.get(last)?.(false); });
    expect(last.querySelector('iframe')).toBeNull();
    expect(mountedWidgetFrameCount()).toBe(MAX_MOUNTED_WIDGET_FRAMES);

    scrolledIntoView.add(count - 1);
    act(() => { reports.get(last)?.(true); });
    expect(last.querySelector('iframe'), 'back on screen, it draws again').not.toBeNull();
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
