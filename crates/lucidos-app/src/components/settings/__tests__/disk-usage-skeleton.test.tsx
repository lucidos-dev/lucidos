// @vitest-environment jsdom
/** Settings › Disk usage draws its figures and worktree rows as skeletons while
 *  its two reads are in flight. One delay gate covers both, so the skeletons
 *  arrive in one wave; each slot still clears on its own read. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render, type FunctionComponent } from 'preact';
import { act } from 'preact/test-utils';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

const answers = new Map<string, (body: unknown) => void>();
let host: HTMLDivElement;
let DiskUsagePage: FunctionComponent;

/** Answers one read, then lets the loader's awaits run out. */
function answer(path: string, body: unknown) {
  return act(async () => {
    answers.get(path)!(body);
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  answers.clear();
  vi.stubGlobal('fetch', (url: string) => new Promise((resolve) => {
    answers.set(url.replace(/^.*\/disk-usage\//, ''), (body) => resolve({
      ok: true,
      status: 200,
      json: async () => body,
    }));
  }));
  vi.resetModules();
  ({ DiskUsagePage } = await import('../DiskUsagePage'));
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<DiskUsagePage />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const bars = (sel: string) => host.querySelectorAll(`${sel} .loading-fade-skeleton .sk-bar`).length;
const shimmering = (el: Element | undefined) =>
  el!.querySelector('.loading-fade-skeleton:not(.loading-fade-out) .sk-bar') !== null;

it('draws its labels at once and no skeleton before the gate opens', () => {
  expect(host.querySelector('.disk-usage-reclaim-title')?.textContent).toBe('Free up space');
  expect(host.querySelector('.disk-usage-key-label')?.textContent).toContain('Worktrees');
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws every owed skeleton together once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  // The headline, three key rows and the cleanup card's figure.
  expect(bars('.disk-usage-figure')).toBe(5);
  expect(host.querySelector('.loading-fade-skeleton .disk-usage-bar .sk-bar')).not.toBeNull();
  expect(host.querySelectorAll('.loading-fade-skeleton .disk-usage-row')).toHaveLength(3);
  expect(host.querySelector('.loading-spinner, .mini-spinner')).toBeNull();
});

it('clears the worktree slots when the inventory lands, while the summary still shimmers', async () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  await answer('worktrees', { worktrees: [] });
  const [worktrees, data] = [...host.querySelectorAll('.disk-usage-key-value')];
  expect(shimmering(worktrees)).toBe(false);
  expect(shimmering(data)).toBe(true);
  expect(shimmering(host.querySelector('.disk-usage-headline-value')!)).toBe(true);
  expect(host.querySelector('.disk-usage-reclaim-figure .loading-fade-content')?.textContent)
    .toBe('Nothing to free right now');
  expect(host.querySelector('.disk-usage-reclaim-btn')).toBeNull();
  expect(host.textContent).toContain('No worktrees on disk');
  expect(host.querySelector('.disk-usage-row')).toBeNull();
});
