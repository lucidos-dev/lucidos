// @vitest-environment jsdom
// A release notice points at a settings page with a link. The body rendered it,
// but no click handler was attached, so the tap went nowhere.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({
  handleNavigationRequest: vi.fn(),
  dismissReleaseNoticeModal: vi.fn(),
  openReleaseNoticesPanel: vi.fn(),
}));

vi.mock('../../../store/actions/navigation-request', () => ({
  handleNavigationRequest: mocks.handleNavigationRequest,
}));
vi.mock('../../../store/actions/releaseNotices', () => ({
  dismissReleaseNoticeModal: mocks.dismissReleaseNoticeModal,
  openReleaseNoticesPanel: mocks.openReleaseNoticesPanel,
}));

import { ReleaseNoticeBody } from '../ReleaseNoticeBody';
import { ReleaseNoticeModal } from '../ReleaseNoticeModal';
import { releaseNoticeView } from '../../../store/store';
import { releaseNoticeDismissed } from '../../../store/releaseNotices';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

const BODY = 'Do it under [Settings > System > Memory](settings:memory). Then **wait**.';

function mount(): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  render(<ReleaseNoticeBody body={BODY} />, host);
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
        notices: [{ id: 'a', since: '1.0.0', title: 'A', body: BODY, resolved: false, action_deferred: false }],
        next_id: 'a',
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

  // The modal announces the notices and leads to the panel. It answers none.
  it('names each owed notice and opens the panel on Read them', () => {
    render(<ReleaseNoticeModal />, host);
    const dialog = document.querySelector('[role="dialog"]')!;
    expect(dialog.textContent).toContain('1 release notice');
    expect([...dialog.querySelectorAll('.release-notice-titles li')].map((li) => li.textContent)).toEqual(['A']);
    dialog.querySelector<HTMLButtonElement>('[data-role="release-notice-open"]')!.click();
    expect(mocks.openReleaseNoticesPanel).toHaveBeenCalledTimes(1);
  });

  it('closes on Later without answering', () => {
    render(<ReleaseNoticeModal />, host);
    document.querySelector<HTMLButtonElement>('[data-role="release-notice-later"]')!.click();
    expect(mocks.dismissReleaseNoticeModal).toHaveBeenCalledTimes(1);
  });
});
