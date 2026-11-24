// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ReleaseNotice } from '../../../api/client';

const submitReleaseNotice = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/releaseNotices', () => ({
  loadReleaseNotices: () => Promise.resolve(),
  submitReleaseNotice,
  releaseNoticeSubmitting: { value: false },
}));

import { ReleaseNoticesPage } from '../ReleaseNoticesPage';
import { releaseNoticeView } from '../../../store/store';

function release(since: string, resolved: boolean, prompt: string | null = null): ReleaseNotice {
  return {
    since,
    items: [{ id: since, since, title: `Item from ${since}`, summary: 'Check it.', body: 'Details.', resolved }],
    resolved,
    action_prompt: prompt,
  };
}

let host: HTMLElement;

beforeEach(() => {
  submitReleaseNotice.mockClear();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  releaseNoticeView.value = { status: 'not-loaded' };
});

it('shows owed releases first and folds the answered history newest first', () => {
  releaseNoticeView.value = {
    status: 'loaded',
    data: {
      releases: [release('1.0.0', true), release('2.0.0', true, 'Check release two.'), release('3.0.0', false)],
      owed_through: '3.0.0',
      action_prompt: null,
    },
  };
  act(() => render(<ReleaseNoticesPage />, host));
  const headings = () => [...host.querySelectorAll('h3')].map((el) => el.textContent);
  expect(headings()).toEqual(['Lucidos 3.0.0: 1 thing for you']);
  expect(host.textContent).toContain('Already answered (2)');
  expect(host.textContent).not.toContain('Check my workspace');
  act(() => host.querySelector<HTMLButtonElement>('.release-notice-answered-toggle')!.click());
  expect(headings()).toEqual([
    'Lucidos 3.0.0: 1 thing for you',
    'Lucidos 2.0.0: 1 thing for you',
    'Lucidos 1.0.0: 1 thing for you',
  ]);
  const check = [...host.querySelectorAll('button')].find((button) => button.textContent === 'Check my workspace')!;
  check.click();
  expect(submitReleaseNotice).toHaveBeenCalledWith(null, 'Check release two.');
});

it('lists only the owed items of a partly answered release, as the modal does', () => {
  const item = (id: string, resolved: boolean) =>
    ({ id, since: '4.0.0', title: `Item ${id}`, summary: 'Check it.', body: 'Details.', resolved });
  releaseNoticeView.value = {
    status: 'loaded',
    data: {
      releases: [{
        since: '4.0.0',
        items: [item('answered-one', true), item('answered-two', true), item('owed', false)],
        resolved: false,
        action_prompt: null,
      }],
      owed_through: 'owed',
      action_prompt: null,
    },
  };
  act(() => render(<ReleaseNoticesPage />, host));
  const owedTitles = () => [...host.querySelectorAll('.release-notice-item-title')].map((el) => el.textContent);
  expect([...host.querySelectorAll('h3')].map((el) => el.textContent)).toEqual(['Lucidos 4.0.0: 1 thing for you']);
  expect(owedTitles()).toEqual(['Item owed']);
  expect(host.textContent).toContain('Already answered (1)');
  act(() => host.querySelector<HTMLButtonElement>('.release-notice-answered-toggle')!.click());
  expect(owedTitles()).toEqual(['Item owed', 'Item answered-one', 'Item answered-two']);
});

it('reports a failed load instead of showing an empty history', () => {
  releaseNoticeView.value = { status: 'failed', error: 'engine unreachable' };
  act(() => render(<ReleaseNoticesPage />, host));
  expect(host.textContent).toContain('engine unreachable');
  expect(host.textContent).not.toContain('Nothing to do');
});
