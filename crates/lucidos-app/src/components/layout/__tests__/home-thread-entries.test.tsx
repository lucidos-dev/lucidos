// @vitest-environment jsdom
/**
 * The home thread's two entries (ADR 0362): the desktop thread header's Home
 * icon and the Lucidos menu's Home row, which only a phone draws. No thread
 * list draws Home, so these are the way to it. Every workspace has Home
 * (ADR 0411), and both show once the thread list carries it.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadHeaderActions, threadHeaderHomeAction } from '../ThreadHeaderActions';
import { HomeMenuGroup } from '../HeaderMark';

function seedThreads(withHome: boolean): void {
  const map = new Map([['other', makeThreadState('other')]]);
  if (withHome) map.set('home', makeThreadState('home', { meta: { title: 'Home', home: true } }));
  threadMap.value = map;
}

beforeEach(() => {
  focusedThreadId.value = null;
});

describe('the desktop thread header', () => {
  it('leads with Home, outside the trailing actions', () => {
    seedThreads(true);
    expect(threadHeaderHomeAction()?.key).toBe('home');
    const order = threadHeaderActions().map((a) => a.key).filter((k) => k !== 'setup-interview');
    expect(order).toEqual(['new-thread', 'search-everywhere']);
  });

  it('carries no Home before the thread list has it', () => {
    seedThreads(false);
    expect(threadHeaderHomeAction()).toBeNull();
  });

  it('opens the home thread', () => {
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

  it('renders nothing, separator included, before the thread list has Home', () => {
    seedThreads(false);
    mount();
    expect(host.innerHTML).toBe('');
  });

  it('closes the menu and opens the home thread', () => {
    seedThreads(true);
    mount();
    const row = host.querySelector<HTMLButtonElement>('button.brand-menu-item');
    expect(row?.textContent).toBe('Home');
    row!.click();
    expect(closed).toBe(1);
    expect(focusedThreadId.value).toBe('home');
  });
});
