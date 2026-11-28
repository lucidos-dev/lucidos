// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const mocks = vi.hoisted(() => ({
  handleNavigationRequest: vi.fn(),
  dismissReleaseNoticeModal: vi.fn(),
  submitReleaseNotice: vi.fn(),
}));

vi.mock('../../../store/actions/navigation-request', () => ({
  handleNavigationRequest: mocks.handleNavigationRequest,
}));
vi.mock('../../../store/actions/releaseNotices', () => ({
  dismissReleaseNoticeModal: () => {
    mocks.dismissReleaseNoticeModal();
    releaseNoticeDismissed.value = true;
  },
  submitReleaseNotice: mocks.submitReleaseNotice,
  releaseNoticeSubmitting: { value: false },
}));

import { ReleaseNoticeBody } from '../ReleaseNoticeBody';
import { ReleaseNoticeModal } from '../ReleaseNoticeModal';
import { releaseNoticeView } from '../../../store/store';
import { releaseNoticeDismissed, releaseNoticeModalOpen } from '../../../store/releaseNotices';
import { _resetOverlayStackForTesting, dismissTopOverlay } from '../../../store/overlayStack';

const BODY = 'Do it under [Settings > System > Memory](settings:memory). Then **wait**.';

function mount(body = BODY): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  render(<ReleaseNoticeBody body={body} />, host);
  return host;
}

describe('a release notice body', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    document.body.innerHTML = '';
  });

  it('opens the settings page its link names', () => {
    const host = mount();
    const link = host.querySelector('a')!;
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith({ target: 'settings', settings_view: 'memory' });
    expect(click.defaultPrevented).toBe(true);
  });
  it('exposes the review link to the shared thread navigation handler', () => {
    const threadId = 'aab2233c-4567-4000-8000-123456789abc';
    const host = mount('[Review trigger grants](thread:' + threadId + ')');
    const link = host.querySelector<HTMLAnchorElement>('a.thread-link')!;
    const click = new MouseEvent('click', { bubbles: true, cancelable: true });
    link.dispatchEvent(click);
    expect(link.dataset.threadId).toBe(threadId);
    expect(click.defaultPrevented).toBe(false);
  });

});

describe('the release notice modal', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    vi.clearAllMocks();
    document.body.innerHTML = '';
    _resetOverlayStackForTesting();
    releaseNoticeDismissed.value = false;
    releaseNoticeView.value = {
      status: 'loaded',
      data: {
        releases: [{
          since: '1.0.0',
          items: ['a', 'b'].map((id) => ({
            id, since: '1.0.0', title: `Item ${id}`, summary: 'Check it.', body: BODY, resolved: false,
          })),
          resolved: false,
          action_prompt: 'Run both checks.',
        }],
        owed_through: 'b',
        action_prompt: 'Run both checks.',
      },
    };
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    releaseNoticeView.value = { status: 'not-loaded' };
  });

  function button(label: string): HTMLButtonElement {
    return [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')]
      .find((button) => button.textContent?.trim() === label)!;
  }

  it('shows one release, with each body initially folded', () => {
    act(() => render(<ReleaseNoticeModal />, host));
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.getAttribute('aria-label')).toBe('Lucidos 1.0.0: 2 things for you');
    expect([...dialog.querySelectorAll('.release-notice-item-title')].map((item) => item.textContent))
      .toEqual(['Item a', 'Item b']);
    expect(dialog.querySelector('.release-notice-body')).toBeNull();
    act(() => button('Read more').click());
    expect(dialog.querySelectorAll('.release-notice-body')).toHaveLength(1);
    expect(dialog.querySelector('.release-notice-body')?.textContent).toContain('Then wait');
  });

  it('answers all displayed items on Got it', () => {
    act(() => render(<ReleaseNoticeModal />, host));
    button('Got it').click();
    expect(mocks.submitReleaseNotice).toHaveBeenCalledWith({ id: 'b', shown_ids: ['a', 'b'] });
  });

  it('runs one prompt for all displayed items', () => {
    act(() => render(<ReleaseNoticeModal />, host));
    button('Check my workspace').click();
    expect(mocks.submitReleaseNotice)
      .toHaveBeenCalledWith({ id: 'b', shown_ids: ['a', 'b'] }, 'Run both checks.');
  });

  it('closes without answering when a detail link opens Settings', () => {
    act(() => render(<ReleaseNoticeModal />, host));
    act(() => button('Read more').click());
    const link = document.querySelector<HTMLAnchorElement>('.release-notice-body a')!;
    act(() => link.click());
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith({ target: 'settings', settings_view: 'memory' });
    expect(mocks.dismissReleaseNoticeModal).toHaveBeenCalledTimes(1);
    expect(mocks.submitReleaseNotice).not.toHaveBeenCalled();
  });

  it.each(['Later', 'Escape', 'X'])('closes on %s without answering', (via) => {
    act(() => render(<ReleaseNoticeModal />, host));
    act(() => {
      if (via === 'Escape') dismissTopOverlay();
      else if (via === 'Later') button('Later').click();
      else document.querySelector<HTMLButtonElement>('[data-role="surface-close"]')!.click();
    });
    expect(mocks.dismissReleaseNoticeModal).toHaveBeenCalledTimes(1);
    expect(mocks.submitReleaseNotice).not.toHaveBeenCalled();
    expect(releaseNoticeModalOpen()).toBe(false);
    releaseNoticeDismissed.value = false;
    expect(releaseNoticeModalOpen()).toBe(true);
  });
});
