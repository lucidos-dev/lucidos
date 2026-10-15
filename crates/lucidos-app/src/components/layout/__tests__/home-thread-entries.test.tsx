// @vitest-environment jsdom
/**
 * The home thread's two entries (ADR 0362): the desktop thread header's Home
 * icon and the Lucidos menu's Home row, which only a phone draws. The Home
 * rail's toggle shares their gate (ADR 0455). No thread
 * list draws Home, so these are the way to it. Every workspace has Home
 * (ADR 0411), and both show once the thread list carries it.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { threadHeaderActions, threadHeaderHomeAction } from '../ThreadHeaderActions';
import { HomeMenuGroup } from '../BrandMenuRows';
import { HomeRailToggle, homeRailToggleOffered } from '../../home/HomeRail';
import { act } from 'preact/test-utils';
import { homeRailShown, setHomeRailShown } from '../../../store/homeRail';

function seedThreads(withHome: boolean): void {
  const map = new Map([['other', makeThreadState('other')]]);
  if (withHome) map.set('home', makeThreadState('home', { meta: { title: 'Home', home: true } }));
  threadMap.value = map;
}

beforeEach(() => {
  focusedThreadId.value = null;
});

describe('the desktop thread header', () => {
  it('leads with Home and New thread, outside the trailing actions', () => {
    seedThreads(true);
    expect(threadHeaderHomeAction()?.key).toBe('home');
    const order = threadHeaderActions().map((a) => a.key).filter((k) => k !== 'setup-interview');
    expect(order).toEqual(['search-everywhere']);
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

describe('the Home rail toggle', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('renders nothing before the thread list has Home', () => {
    seedThreads(false);
    render(<HomeRailToggle />, host);
    expect(host.innerHTML).toBe('');
    expect(document.documentElement.hasAttribute('data-home-rail-toggle')).toBe(false);
  });

  it('shows once Home exists, and shows and hides the rail', () => {
    seedThreads(true);
    setHomeRailShown(true);
    render(<HomeRailToggle />, host);
    const button = host.querySelector<HTMLButtonElement>('button.home-rail-toggle')!;
    expect(button.getAttribute('aria-label')).toBe('Hide Home board');
    expect(button.getAttribute('aria-pressed')).toBe('true');
    expect(document.documentElement.hasAttribute('data-home-rail-toggle')).toBe(true);
    act(() => button.click());
    expect(homeRailShown.value).toBe(false);
    expect(button.getAttribute('aria-label')).toBe('Show Home board');
  });

  it('is offered while the rail shows, and while hidden only for a board with tiles', () => {
    const tiles = (n: number) => ({
      status: 'loaded' as const,
      data: { tiles: Array.from({ length: n }, () => ({}) as never) } as never,
    });
    expect(homeRailToggleOffered(true, { status: 'not-loaded' })).toBe(true);
    expect(homeRailToggleOffered(true, tiles(0))).toBe(true);
    expect(homeRailToggleOffered(false, { status: 'not-loaded' })).toBe(false);
    expect(homeRailToggleOffered(false, tiles(0))).toBe(false);
    expect(homeRailToggleOffered(false, tiles(1))).toBe(true);
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
