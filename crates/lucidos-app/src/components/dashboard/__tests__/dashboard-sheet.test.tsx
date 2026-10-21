// @vitest-environment jsdom
/**
 * The dashboard sheet: the phone dashboard over the open thread. Its toggle
 * sits in the thread title row, on a phone only. Opening never moves focus.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../Dashboard', () => ({ Dashboard: () => <div data-stub="dashboard" /> }));

import { DashboardSheet } from '../DashboardSheet';
import { MobileThreadTitleBar } from '../../layout/MobileAppHeader';
import { DashboardRail } from '../DashboardRail';
import { closeDashboardSheet, dashboardSheetOpen } from '../../../store/dashboards';
import { focusedThreadId, mobileView, setMobileView, threadMap } from '../../../store/store';
import { revealDashboard } from '../../../store/actions/dashboardNav';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { _resetOverlayStackForTesting, topPanelOverlay } from '../../../store/overlayStack';
import { viewportIsMobile } from '../../../utils/viewport';

let host: HTMLDivElement;

const toggle = () => host.querySelector<HTMLButtonElement>('.mobile-thread-title-row .dashboard-sheet-toggle');
const sheet = () => document.querySelector<HTMLElement>('.dashboard-sheet');
const panel = () => document.querySelector<HTMLElement>('.dashboard-sheet-panel');

function mountPhone(): void {
  viewportIsMobile.value = true;
  threadMap.value = new Map([['t1', makeThreadState('t1', { meta: { title: 'A thread', section: 'inbox' }, eventsLoaded: true })]]);
  focusedThreadId.value = 't1';
  act(() => render(<><MobileThreadTitleBar /><DashboardSheet /></>, host));
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  closeDashboardSheet();
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  focusedThreadId.value = null;
  threadMap.value = new Map();
  viewportIsMobile.value = false;
  closeDashboardSheet();
  _resetOverlayStackForTesting();
});

describe('the title row button', () => {
  it('shows on a phone thread, labelled and unpressed while the sheet is closed', () => {
    mountPhone();
    const button = toggle();
    expect(button).not.toBeNull();
    expect(button!.getAttribute('aria-label')).toBe('Show dashboard');
    expect(button!.getAttribute('aria-pressed')).toBe('false');
  });

  it('shows on Home as on any thread', () => {
    viewportIsMobile.value = true;
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true, section: 'inbox' }, eventsLoaded: true })]]);
    focusedThreadId.value = 'home';
    act(() => render(<MobileThreadTitleBar />, host));
    expect(toggle()).not.toBeNull();
  });

  it('is absent outside the phone layout', () => {
    mountPhone();
    act(() => { viewportIsMobile.value = false; });
    expect(toggle()).toBeNull();
  });

  it('presses and opens the sheet, and a second tap closes it', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(dashboardSheetOpen.value).toBe(true);
    expect(toggle()!.getAttribute('aria-pressed')).toBe('true');
    expect(toggle()!.getAttribute('aria-label')).toBe('Hide dashboard');
    act(() => toggle()!.click());
    expect(dashboardSheetOpen.value).toBe(false);
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

describe('the sheet', () => {
  it('stays unmounted until the first open', () => {
    mountPhone();
    expect(sheet()).toBeNull();
  });

  it('opens over the thread without moving focus', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(focusedThreadId.value).toBe('t1');
    expect(sheet()!.classList.contains('is-open')).toBe(true);
    expect(panel()!.hasAttribute('inert')).toBe(false);
  });

  it('stays mounted, inert and hidden once closed, so a reopen finds its tiles', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => toggle()!.click());
    expect(sheet()).not.toBeNull();
    expect(sheet()!.classList.contains('is-open')).toBe(false);
    expect(panel()!.hasAttribute('inert')).toBe(true);
    expect(panel()!.getAttribute('aria-hidden')).toBe('true');
  });

  it('stamps its panel with the top overlay, so a tile frame inside it may open its menu', () => {
    mountPhone();
    act(() => toggle()!.click());
    expect(panel()!.getAttribute('data-overlay-panel')).toBe(topPanelOverlay()!.id);
  });

  it('closes on Escape', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    expect(dashboardSheetOpen.value).toBe(false);
  });
});

describe('what ends the sheet', () => {
  afterEach(() => setMobileView('thread'));

  it('closes when the pane changes, so it never covers a pane without a title row', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => setMobileView('content'));
    expect(dashboardSheetOpen.value).toBe(false);
  });

  it('closes when the thread changes by any route', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => { focusedThreadId.value = null; });
    expect(dashboardSheetOpen.value).toBe(false);
  });

  it('closes when the viewport leaves the phone layout, and stays closed on return', () => {
    mountPhone();
    act(() => toggle()!.click());
    act(() => { viewportIsMobile.value = false; });
    act(() => { viewportIsMobile.value = true; });
    expect(dashboardSheetOpen.value).toBe(false);
  });

  it('stays open when revealed from another pane, which it first leaves for the thread pane', () => {
    mountPhone();
    setMobileView('content');
    act(() => revealDashboard());
    expect(mobileView.value).toBe('thread');
    expect(dashboardSheetOpen.value).toBe(true);
  });
});
