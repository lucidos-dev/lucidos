// @vitest-environment jsdom
/**
 * The filtered drawer views (In flight, Blocked, Drafts) still list the
 * home thread when it qualifies. Its row there draws a house and no pin: the
 * engine refuses a pin on Home (ADR 0362).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { ThreadRow } from '../ThreadDrawer';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

vi.mock('../../../store/actions/threads', () => ({
  focusThread: vi.fn(),
  handleSaveThread: vi.fn(),
  handleUnsaveThread: vi.fn(),
}));
vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

let host: HTMLDivElement;

function mount(home: boolean): void {
  const thread = makeThreadState('t', { meta: { title: 'Home', section: 'inbox', ...(home ? { home: true } : {}) } });
  threadMap.value = new Map([['t', thread]]);
  render(<ThreadRow threadId="t" status="idle" />, host);
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe("the home thread's row in a filtered view", () => {
  it('draws a house and no pin', () => {
    mount(true);
    expect(host.querySelector('.thread-row-home-icon svg')).not.toBeNull();
    expect(host.querySelector('.pin-thread-btn')).toBeNull();
  });

  it('leaves the pin on every other row', () => {
    mount(false);
    expect(host.querySelector('.thread-row-home-icon')).toBeNull();
    expect(host.querySelector('.pin-thread-btn')).not.toBeNull();
  });
});
