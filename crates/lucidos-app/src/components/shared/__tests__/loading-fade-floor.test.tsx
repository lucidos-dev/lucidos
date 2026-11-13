// @vitest-environment jsdom
/** A skeleton the delay gate let through stays opaque for LOADER_MIN_VISIBLE_MS,
 *  hides its loaded content meanwhile, then fades. A load that outlasts the floor
 *  ends the skeleton at once. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { LoadingFade } from '../LoadingFade';
import { LOADER_MIN_VISIBLE_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

const show = (showSkeleton: boolean) =>
  act(() => {
    render(
      <LoadingFade showSkeleton={showSkeleton} skeleton={<div class="test-skeleton" />}>
        <div class="loaded" />
      </LoadingFade>,
      host,
    );
  });

const skeleton = () => host.querySelector('.loading-fade-skeleton');
const isFadingOut = () => skeleton()?.classList.contains('loading-fade-out') === true;
const contentHidden = () => host.querySelector('.loading-fade')!.classList.contains('loading-fade-held');

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

describe('LoadingFade minimum-visible floor', () => {
  it('keeps the skeleton opaque and the content hidden until the floor elapses', () => {
    show(true);
    act(() => { vi.advanceTimersByTime(30); });
    show(false); // the load ended 30 ms after the skeleton appeared
    expect(skeleton()).not.toBeNull();
    expect(isFadingOut()).toBe(false);
    expect(contentHidden()).toBe(true);

    act(() => { vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS - 30 - 1); });
    expect(isFadingOut()).toBe(false);
    expect(contentHidden()).toBe(true);

    act(() => { vi.advanceTimersByTime(1); });
    expect(isFadingOut()).toBe(true);
    expect(contentHidden()).toBe(false);
  });

  it('does not hold a load that outlasted the floor', () => {
    show(true);
    act(() => { vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS * 4); });
    show(false);
    expect(isFadingOut()).toBe(true);
    expect(contentHidden()).toBe(false);
  });

  it('draws no skeleton and hides nothing for a loader that never appeared', () => {
    show(false);
    expect(skeleton()).toBeNull();
    expect(contentHidden()).toBe(false);
  });
});
