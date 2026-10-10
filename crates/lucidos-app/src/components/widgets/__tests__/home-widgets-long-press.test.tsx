// @vitest-environment jsdom
/**
 * A hold or a right-click on a Home entry lists Home's pinned widgets
 * (ADR 0419). A pick floats that widget in a window. A tap still opens Home
 * and nothing else. Rendered, because the gesture, the swallowed click and the
 * menu only exist together.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import type { ThreadWidget } from '../../../api/client/widgets';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadWidgets } from '../../../store/widgets';
import { widgetWindows } from '../../../store/widgetWindows';
import { ThreadHeaderHomeButton } from '../../layout/ThreadHeaderActions';
import { HomeMenuGroup } from '../../layout/HeaderMark';
import { NO_HOME_WIDGETS } from '../HomeWidgetsMenu';
import { widgetInstanceKey } from '../../../utils/widgetParams';

let host: HTMLDivElement;

function widget(appId: string, pinned: boolean): ThreadWidget {
  return { app_id: appId, name: appId, reusable: false, reveal: 'on-load', pinned, shown_event_id: `ev-${appId}` };
}

function setWidgets(threadId: string, data: ThreadWidget[]): void {
  threadWidgets.value = new Map([...threadWidgets.value, [threadId, { status: 'loaded', data }]]);
}

function pointer(type: string): PointerEvent {
  // jsdom has no PointerEvent constructor. `useLongPress` reads only `button`
  // and `clientX/Y`.
  return new MouseEvent(type, { bubbles: true, cancelable: true, button: 0 }) as unknown as PointerEvent;
}

async function flush(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

async function hold(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  vi.advanceTimersByTime(500);
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

async function tap(target: HTMLElement): Promise<void> {
  target.dispatchEvent(pointer('pointerdown'));
  target.dispatchEvent(pointer('pointerup'));
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  await flush();
}

const menu = () => document.querySelector<HTMLElement>('.thread-overflow-menu');
/** Each row's label. The fixtures name a widget after its app id. */
const rows = () => [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [data-home-widget]')]
  .map((el) => el.querySelector('.thread-overflow-label')?.textContent);
const k = (appId: string) => widgetInstanceKey(appId, undefined);
const row = (key: string) => document.querySelector<HTMLElement>(`[data-home-widget="${CSS.escape(key)}"]`)!;
const homeButton = () => host.querySelector<HTMLElement>('.home-thread-btn')!;

beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  widgetWindows.value = [];
  threadMap.value = new Map([
    ['other', makeThreadState('other')],
    ['home', makeThreadState('home', { meta: { title: 'Home', home: true } })],
  ]);
  focusedThreadId.value = 'other';
  threadWidgets.value = new Map();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  vi.useRealTimers();
});

describe('the Home icon', () => {
  it('opens Home on a tap, and no menu', async () => {
    setWidgets('home', [widget('fares', true)]);
    render(<ThreadHeaderHomeButton />, host);
    await tap(homeButton());
    expect(focusedThreadId.value).toBe('home');
    expect(menu()).toBeNull();
  });

  it('lists Home\'s pinned widgets on a hold, and stays where it is', async () => {
    setWidgets('home', [widget('fares', true), widget('notes', false), widget('weather', true)]);
    setWidgets('other', [widget('elsewhere', true)]);
    render(<ThreadHeaderHomeButton />, host);
    await hold(homeButton());
    expect(rows()).toEqual(['fares', 'weather']);
    expect(focusedThreadId.value).toBe('other');
  });

  it('opens the same menu on a right-click', async () => {
    setWidgets('home', [widget('fares', true)]);
    render(<ThreadHeaderHomeButton />, host);
    homeButton().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 20, clientY: 20 }));
    await flush();
    expect(rows()).toEqual(['fares']);
    expect(focusedThreadId.value).toBe('other');
  });

  it('follows a pin made while the menu is open', async () => {
    setWidgets('home', [widget('fares', true)]);
    render(<ThreadHeaderHomeButton />, host);
    await hold(homeButton());
    setWidgets('home', [widget('fares', true), widget('weather', true)]);
    await flush();
    expect(rows()).toEqual(['fares', 'weather']);
  });

  it('lists each instance of a widget pinned twice, and opens the one picked', async () => {
    setWidgets('home', [
      { ...widget('habits', true), params: { range: 'week' }, label: 'This week' },
      { ...widget('habits', true), params: { range: 'month' }, label: 'This month' },
    ]);
    render(<ThreadHeaderHomeButton />, host);
    await hold(homeButton());
    expect(rows()).toEqual(['This week', 'This month']);
    row(widgetInstanceKey('habits', { range: 'month' })).click();
    await flush();
    expect(widgetWindows.value.map((w) => w.instanceKey)).toEqual([widgetInstanceKey('habits', { range: 'month' })]);
  });

  it('says so when nothing is pinned to Home', async () => {
    setWidgets('home', [widget('notes', false)]);
    render(<ThreadHeaderHomeButton />, host);
    await hold(homeButton());
    expect(rows()).toEqual([]);
    expect(menu()?.querySelector('.thread-overflow-note')?.textContent).toBe(NO_HOME_WIDGETS);
  });

  it('floats a picked widget in a window, without leaving the thread', async () => {
    setWidgets('home', [widget('fares', true)]);
    render(<ThreadHeaderHomeButton />, host);
    await hold(homeButton());
    row(k('fares')).click();
    await flush();
    expect(widgetWindows.value.map((w) => [w.threadId, w.instanceKey])).toEqual([['home', k('fares')]]);
    expect(focusedThreadId.value).toBe('other');
    expect(menu()).toBeNull();
  });
});

describe('the Lucidos menu Home row', () => {
  it('lists Home\'s widgets on a hold, and a pick closes the Lucidos menu too', async () => {
    setWidgets('home', [widget('fares', true)]);
    let closed = 0;
    render(<HomeMenuGroup onClose={() => { closed += 1; }} />, host);
    await hold(homeButton());
    expect(closed).toBe(0);
    expect(rows()).toEqual(['fares']);
    row(k('fares')).click();
    await flush();
    expect(closed).toBe(1);
    expect(widgetWindows.value.map((w) => w.instanceKey)).toEqual([k('fares')]);
    expect(focusedThreadId.value).toBe('other');
  });
});
