// @vitest-environment jsdom
/**
 * The widget shelf draws a chip per pinned widget in a thread's title row.
 * Home draws none: it takes no pins, since its widgets go on a dashboard
 * (ADR 0496).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ThreadWidget } from '../../../api/client/widgets';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { OpenShelfWidget, WidgetShelf } from '../WidgetShelf';
import { WidgetMenuItems } from '../widgetMenuChunks';
import { APP_FRAME_MENU_EVENT } from '../../../store/actions/widget-frame-bridge';

let host: HTMLDivElement;

function widget(appId: string): ThreadWidget {
  return { app_id: appId, name: appId, reusable: false, reveal: 'on-load', sizes: ['medium'], pinned: true, shown_event_id: `ev-${appId}` };
}

const chips = () => [...host.querySelectorAll<HTMLElement>('[data-widget-chip]')].map((el) => el.dataset.widgetChip);

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  threadMap.value = new Map([
    ['home', makeThreadState('home', { meta: { title: 'Home', home: true } })],
    ['t1', makeThreadState('t1', { meta: { title: 'Trip' } })],
  ]);
  threadWidgets.value = new Map([
    ['home', { status: 'loaded', data: [widget('cost')] }],
    ['t1', { status: 'loaded', data: [widget('fares')] }],
  ]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
});

describe('the widget shelf', () => {
  it('draws a chip per pinned widget in a thread', () => {
    act(() => { render(<WidgetShelf threadId="t1" />, host); });
    expect(chips()).toEqual(['fares']);
  });

  it('draws nothing on Home', () => {
    act(() => { render(<WidgetShelf threadId="home" />, host); });
    expect(host.querySelector('.widget-shelf')).toBeNull();
  });
});

describe('the open shelf widget', () => {
  beforeEach(async () => {
    await WidgetMenuItems.preload();
    act(() => { render(<OpenShelfWidget threadId="t1" entry={widget('fares')} onClose={() => {}} />, host); });
  });

  const menuItems = () => [...document.querySelectorAll('.thread-overflow-menu [role="menuitem"]')].map((b) => b.textContent);

  it("a right-click on its panel opens the widget's menu", () => {
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 30, clientY: 40 });
    act(() => { host.querySelector('.widget-bar')!.dispatchEvent(e); });
    expect(e.defaultPrevented).toBe(true);
    expect(menuItems()).toContain('Unpin from shelf');
  });

  it("a right-click its frame forwards opens the same menu", () => {
    act(() => {
      host.querySelector('iframe')!.dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { detail: { x: 30, y: 40 }, bubbles: true }));
    });
    expect(menuItems()).toContain('Unpin from shelf');
  });
});
