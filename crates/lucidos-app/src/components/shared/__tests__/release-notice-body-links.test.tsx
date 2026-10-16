// @vitest-environment jsdom
// A release notice points at a settings page with a link. The body rendered it,
// but no click handler was attached, so the tap went nowhere.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

const mocks = vi.hoisted(() => ({
  handleNavigationRequest: vi.fn(),
  acknowledgeReleaseNotice: vi.fn(),
  dismissReleaseNoticeModal: vi.fn(),
}));

vi.mock('../../../store/actions/navigation-request', () => ({
  handleNavigationRequest: mocks.handleNavigationRequest,
}));
vi.mock('../../../store/actions/releaseNotices', () => ({
  acknowledgeReleaseNotice: mocks.acknowledgeReleaseNotice,
  dismissReleaseNoticeModal: mocks.dismissReleaseNoticeModal,
  takeReleaseNoticeAction: vi.fn(),
}));

import { ReleaseNoticeBody } from '../ReleaseNoticeBody';
import { ReleaseNoticeModal } from '../ReleaseNoticeModal';
import { releaseNoticeView } from '../../../store/store';
import { releaseNoticeDismissed } from '../../../store/releaseNotices';
import { _resetOverlayStackForTesting } from '../../../store/overlayStack';

const BODY = 'Do it under [Settings > System > Memory](settings:memory). Then **wait**.';

function mount(onLinkClick?: () => void): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  render(<ReleaseNoticeBody body={BODY} onLinkClick={onLinkClick} />, host);
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

  // The modal passes its dismiss here: the page the link opens would otherwise
  // sit inert behind the overlay.
  it('runs onLinkClick after a link, and only after a link', () => {
    const onLinkClick = vi.fn();
    const host = mount(onLinkClick);
    host.querySelector('strong')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(onLinkClick).not.toHaveBeenCalled();
    host.querySelector('a')!.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(onLinkClick).toHaveBeenCalledTimes(1);
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

  // An overlay leaves the page behind it inert, so the Memory page a link opens
  // would be untouchable under a modal left open.
  it('closes on a body link without answering the notice', () => {
    render(<ReleaseNoticeModal />, host);
    const link = document.querySelector<HTMLAnchorElement>('[role="dialog"] .release-notice-body a')!;
    link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(mocks.handleNavigationRequest).toHaveBeenCalledWith({ target: 'settings', settings_view: 'memory' });
    expect(mocks.dismissReleaseNoticeModal).toHaveBeenCalledTimes(1);
    expect(mocks.acknowledgeReleaseNotice).not.toHaveBeenCalled();
  });
});
