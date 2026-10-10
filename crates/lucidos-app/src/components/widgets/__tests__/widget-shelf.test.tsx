// @vitest-environment jsdom
/**
 * The widget shelf draws a chip per pinned widget in a thread's title row.
 * Home draws none: its pinned widgets live in the Home long-press menu and
 * their windows (ADR 0419), so a chip there would repeat them.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ThreadWidget } from '../../../api/client/widgets';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { WidgetShelf } from '../WidgetShelf';

let host: HTMLDivElement;

function widget(appId: string): ThreadWidget {
  return { app_id: appId, name: appId, reusable: false, reveal: 'on-load', pinned: true, shown_event_id: `ev-${appId}` };
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
