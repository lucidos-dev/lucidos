// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../chat/PromptInput', () => ({ PromptInput: () => <textarea class="prompt-textarea" /> }));
vi.mock('../../chat/FollowToggle', () => ({ FollowLiveEdgeToggle: () => null }));
vi.mock('../../chat/ThreadView', () => ({ ThreadView: () => <div data-stub="thread-view" /> }));
vi.mock('../../chat/CreateThreadView', () => ({ CreateThreadView: () => <div data-stub="compose-view" /> }));
vi.mock('../../chat/HomeWelcomeTitle', () => ({ HomeWelcomeTitle: () => <div data-stub="home-title" /> }));

const phone = vi.hoisted(() => ({ on: false }));
vi.mock('../../../utils/viewport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/viewport')>()),
  isMobile: () => phone.on,
}));

import { ThreadPane } from '../ThreadPane';
import {
  animationSpeed,
  focusedThreadId,
  promptAnimating,
  promptSendCollapsing,
  threadMap,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

let host: HTMLDivElement;

const openThread = () => act(() => { focusedThreadId.value = 't1'; });
const openCompose = () => act(() => { focusedThreadId.value = null; });
const frames = () => new Promise<void>((resolve) => {
  requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
});

const promptArea = () => host.querySelector<HTMLElement>('.prompt-area')!;
const textarea = () => host.querySelector<HTMLTextAreaElement>('.prompt-textarea')!;

function mountCompose(): void {
  render(<ThreadPane />, host);
  promptArea().getBoundingClientRect = () =>
    ({ top: host.querySelector('.thread-view-clip') ? 300 : 100 }) as DOMRect;
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { title: 'Thread' } })]]);
  focusedThreadId.value = null;
  phone.on = false;
});

afterEach(() => {
  render(null, host);
  host.remove();
  focusedThreadId.value = null;
  threadMap.value = new Map();
  phone.on = false;
  promptSendCollapsing.value = false;
  promptAnimating.value = false;
  animationSpeed.value = 0;
});

describe('the compose-to-thread prompt slide', () => {
  it('runs at the speed-slider duration, not the 1x base', async () => {
    animationSpeed.value = 10;
    mountCompose();
    await openThread();
    await frames();
    expect(promptArea().style.transition).toContain('transform 30ms ease');
  });

  it('releases a slide whose transition never ends on the speed-slider timer', async () => {
    animationSpeed.value = 10;
    mountCompose();
    await openThread();
    expect(promptAnimating.value).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(promptAnimating.value).toBe(false);
  });

  it('does not slide the prompt on a phone', async () => {
    phone.on = true;
    mountCompose();
    await openThread();
    expect(promptArea().style.transform).toBe('');
  });

  it('still releases the deferred textarea height on a phone', async () => {
    phone.on = true;
    mountCompose();
    textarea().style.height = '200px';
    promptSendCollapsing.value = true;
    await openThread();
    expect(textarea().style.height).toBe('');
  });

  it('animates the collapse from the height the textarea had before submit', async () => {
    mountCompose();
    Object.defineProperty(textarea(), 'offsetHeight', {
      configurable: true,
      get: () => (host.querySelector('.thread-view-clip') ? 40 : 200),
    });
    promptSendCollapsing.value = true;
    await openThread();
    expect(textarea().style.height).toBe('200px');
  });

  it('keeps a draft textarea height when the prompt only moves into it', async () => {
    mountCompose();
    await openThread();
    const ta = textarea();
    ta.style.height = '120px';
    await openCompose();
    render(null, host);
    expect(ta.style.height).toBe('120px');
  });
});
