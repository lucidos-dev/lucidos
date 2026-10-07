// @vitest-environment jsdom
/**
 * The home thread's two entries (ADR 0362): the desktop thread header's Home
 * icon and the Lucidos menu's Home row, which only a phone draws. No thread
 * list draws Home, so these are the way to it. Both show only while the
 * experimental switch is on and the thread exists.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { focusedThreadId, preferences, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadHeaderActions, threadHeaderHomeAction } from '../ThreadHeaderActions';
import { HomeMenuGroup } from '../HeaderMark';

function switchHome(on: boolean): void {
  preferences.value = { status: 'loaded', data: { home_thread_enabled: on ? 'true' : 'false' } };
}

function seedThreads(withHome: boolean): void {
  const map = new Map([['other', makeThreadState('other')]]);
  if (withHome) map.set('home', makeThreadState('home', { meta: { title: 'Home', home: true } }));
  threadMap.value = map;
}

beforeEach(() => {
  focusedThreadId.value = null;
});

describe('the desktop thread header', () => {
  it('leads with Home, outside the trailing actions, while the switch is on', () => {
    switchHome(true);
    seedThreads(true);
    expect(threadHeaderHomeAction()?.key).toBe('home');
    const order = threadHeaderActions().map((a) => a.key).filter((k) => k !== 'setup-interview');
    expect(order).toEqual(['new-thread', 'search-everywhere']);
  });

  it('carries no Home while the switch is off', () => {
    switchHome(false);
    seedThreads(true);
    expect(threadHeaderHomeAction()).toBeNull();
  });

  it('carries no Home before the thread exists', () => {
    switchHome(true);
    seedThreads(false);
    expect(threadHeaderHomeAction()).toBeNull();
  });

  it('opens the home thread', () => {
    switchHome(true);
    seedThreads(true);
    threadHeaderHomeAction()!.onClick!(new MouseEvent('click'));
    expect(focusedThreadId.value).toBe('home');
  });
});

describe('the Lucidos menu Home row', () => {
  let host: HTMLDivElement;
  let closed: number;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    closed = 0;
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  const mount = () => render(<HomeMenuGroup onClose={() => { closed += 1; }} />, host);

  it('renders nothing, separator included, while the switch is off', () => {
    switchHome(false);
    seedThreads(true);
    mount();
    expect(host.innerHTML).toBe('');
  });

  it('closes the menu and opens the home thread', () => {
    switchHome(true);
    seedThreads(true);
    mount();
    const row = host.querySelector<HTMLButtonElement>('button.brand-menu-item');
    expect(row?.textContent).toBe('Home');
    row!.click();
    expect(closed).toBe(1);
    expect(focusedThreadId.value).toBe('home');
  });
});
