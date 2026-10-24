// @vitest-environment jsdom
/**
 * The dashboard panel: the phone dashboard over the open thread. Its toggle
 * sits beside the mark in the thread header, on a phone only. Opening never
 * moves focus.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../Dashboard', () => ({
  Dashboard: ({ hidden }: { hidden?: boolean }) => <div data-stub="dashboard" data-hidden={String(hidden)} />,
}));

import { DashboardPanel } from '../DashboardPanel';
import { MOBILE_PANE_CONFIGS } from '../../layout/MobileAppHeader';
import { DashboardRail } from '../DashboardRail';
import { closeDashboardPanel, dashboardPanelOpen } from '../../../store/dashboards';
import { focusedThreadId, mobileView, setMobileView, threadMap } from '../../../store/store';
import { revealDashboard } from '../../../store/actions/dashboardNav';
import { _resetThreadNavForTesting, threadNavHistory } from '../../../store/actions/thread-navigation';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { _resetOverlayStackForTesting, topPanelOverlay } from '../../../store/overlayStack';
import { viewportIsMobile } from '../../../utils/viewport';

let host: HTMLDivElement;

const ThreadHeader = MOBILE_PANE_CONFIGS.thread.Header;
const toggle = () => host.querySelector<HTMLButtonElement>('.mobile-thread-header .header-mark-group .dashboard-panel-toggle');
const panel = () => document.querySelector<HTMLElement>('.dashboard-panel');
const body = () => document.querySelector<HTMLElement>('.dashboard-panel-body');

function mountPhone(): void {
  viewportIsMobile.value = true;
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { title: 'A thread', section: 'inbox' }, eventsLoaded: true })]]);
  focusedThreadId.value = 't1';
  act(() => render(<><ThreadHeader /><DashboardPanel /></>, host));
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  closeDashboardPanel();
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  focusedThreadId.value = null;
  threadMap.value = new Map();
  viewportIsMobile.value = false;
  closeDashboardPanel();
  _resetOverlayStackForTesting();
});

describe('the header button', () => {
  it('shows on a phone thread, labelled and unpressed while the panel is closed', () => {
    mountPhone();
    const button = toggle();
    expect(button).not.toBeNull();
    expect(button!.getAttribute('aria-label')).toBe('Show dashboard');
    expect(button!.getAttribute('aria-pressed')).toBe('false');
  });

  it('shows with no thread focused, and opens the panel there', () => {
    viewportIsMobile.value = true;
    act(() => render(<><ThreadHeader /><DashboardPanel /></>, host));
    expect(toggle()).not.toBeNull();
    act(() => toggle()!.click());
    expect(dashboardPanelOpen.value).toBe(true);
  });

  it('is absent outside the phone layout', () => {
    mountPhone();
    act(() => { viewportIsMobile.value = false; });
    expect(toggle()).toBeNull();
  });

  it('presses and opens the panel, and a second tap closes it', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(dashboardPanelOpen.value).toBe(true);
    expect(toggle()!.getAttribute('aria-pressed')).toBe('true');
    expect(toggle()!.getAttribute('aria-label')).toBe('Hide dashboard');
    act(() => toggle()!.click());
    expect(dashboardPanelOpen.value).toBe(false);
    expect(toggle()!.getAttribute('aria-pressed')).toBe('false');
  });

  it('draws the same icon as the rail toggle', () => {
    mountPhone();
    const phoneSvg = toggle()!.querySelector('svg')!.innerHTML;
    viewportIsMobile.value = false;
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true, section: 'inbox' }, eventsLoaded: true })]]);
    const railHost = document.createElement('div');
    document.body.appendChild(railHost);
    act(() => render(<DashboardRail />, railHost));
    const railSvg = railHost.querySelector('.dashboard-rail-toggle svg')?.innerHTML;
    act(() => render(null, railHost));
    railHost.remove();
    expect(railSvg).toBe(phoneSvg);
  });
});

describe('the panel', () => {
  it('stays unmounted until the first open', () => {
    mountPhone();
    expect(panel()).toBeNull();
  });

  it('opens over the thread without moving focus', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(focusedThreadId.value).toBe('t1');
    expect(panel()!.classList.contains('is-open')).toBe(true);
    expect(body()!.hasAttribute('inert')).toBe(false);
  });

  it('stays mounted, inert and hidden once closed, so a reopen finds its tiles', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => toggle()!.click());
    expect(panel()).not.toBeNull();
    expect(panel()!.classList.contains('is-open')).toBe(false);
    expect(body()!.hasAttribute('inert')).toBe(true);
    expect(body()!.getAttribute('aria-hidden')).toBe('true');
  });

  it('pushes no overlay, so the header stays live and an outside tap is an ordinary tap', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(topPanelOverlay()).toBeNull();
    expect(body()!.hasAttribute('data-overlay-panel')).toBe(false);
  });

  it('carries its own edge zones, so a touch at the screen edge never reaches a tile frame', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(panel()!.querySelectorAll('.edge-swipe-zone')).toHaveLength(2);
  });
});

describe('what ends the panel', () => {
  afterEach(() => setMobileView('thread'));

  it('stays open across a pane swipe and back', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => setMobileView('content'));
    expect(dashboardPanelOpen.value).toBe(true);
    act(() => setMobileView('thread'));
    expect(dashboardPanelOpen.value).toBe(true);
  });

  it('closes when the thread changes by any route', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => { focusedThreadId.value = null; });
    expect(dashboardPanelOpen.value).toBe(false);
  });

  it('folds its rows out of the thread history when the viewport leaves the phone layout', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(threadNavHistory.value.stack.some((row) => row.type === 'dashboard')).toBe(true);
    act(() => { viewportIsMobile.value = false; });
    expect(threadNavHistory.value.stack.some((row) => row.type === 'dashboard')).toBe(false);
  });

  /** Invariant: a cold launch that reads the layout wrong at first still reopens the panel the history points at. */
  it('reopens the panel the history points at when the layout turns phone after startup', () => {
    act(() => { viewportIsMobile.value = false; });
    threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { title: 'A thread', section: 'inbox' }, eventsLoaded: true })]]);
    focusedThreadId.value = 't1';
    _resetThreadNavForTesting();
    localStorage.setItem('lucidos-thread-nav-history', JSON.stringify({
      stack: [{ type: 'thread', id: 't1' }, { type: 'dashboard', id: 't1' }],
      cursor: 1,
    }));
    act(() => { viewportIsMobile.value = true; });
    expect(dashboardPanelOpen.value).toBe(true);
  });

  it('closes when the viewport leaves the phone layout, and stays closed on return', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => { viewportIsMobile.value = false; });
    act(() => { viewportIsMobile.value = true; });
    expect(dashboardPanelOpen.value).toBe(false);
  });

  /** Invariant: the dashboard counts as hidden off the thread pane, so it settles only once that pane has slid in. */
  it('tells its dashboard it is hidden while the thread pane is not the current pane', async () => {
    mountPhone();
    act(() => setMobileView('content'));
    act(() => toggle()!.click());
    const stub = () => document.querySelector<HTMLElement>('[data-stub="dashboard"]');
    await vi.waitFor(() => expect(stub()).not.toBeNull());
    expect(stub()!.dataset.hidden).toBe('true');
    act(() => setMobileView('thread'));
    expect(stub()!.dataset.hidden).toBe('false');
  });

  it('opens when revealed from another pane, which it first leaves for the thread pane', () => {
    mountPhone();
    setMobileView('content');
    act(() => revealDashboard());
    expect(mobileView.value).toBe('thread');
    expect(dashboardPanelOpen.value).toBe(true);
  });
});

describe('the panel module loading late', () => {
  it('keeps a panel the startup already reopened', async () => {
    // The client starts from the entry chunk, so the shell chunk holding this
    // module can load after a reload has reopened the panel.
    vi.resetModules();
    const { openDashboardPanel, dashboardPanelOpen: openSignal } = await import('../../../store/dashboards');
    openDashboardPanel();
    await import('../DashboardPanel');
    expect(openSignal.value).toBe(true);
  });
});
