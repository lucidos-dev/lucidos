// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../store/actions/threadHold', () => ({ resumeHeldThread: vi.fn() }));
vi.mock('../../../store/actions/threads', () => ({ focusThread: vi.fn() }));

import { HoldStrip, holdPlacer, holdWaiting } from '../HoldStrip';
import { resumeHeldThread } from '../../../store/actions/threadHold';
import { focusThread } from '../../../store/actions/threads';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { ThreadHoldSummary } from '../../../api/threads';
import type { ThreadState } from '../../../store/thread-events';

const PLACED_AT = '2026-04-12T09:00:00Z';

function heldOn(root: string, placedBy?: ThreadHoldSummary['placed_by']): ThreadHoldSummary {
  return { reason: 'Waiting on the vendor', root_thread_id: root, placed_at: PLACED_AT, placed_by: placedBy };
}

function threads(...list: ThreadState[]): Map<string, ThreadState> {
  return new Map(list.map((t) => [t.meta.id, t]));
}

describe('holdPlacer', () => {
  it('names the owner, an agent thread, or Lucidos', () => {
    const agent = makeThreadState('agent', { meta: { title: 'Ship the fix' } });
    const map = threads(agent);
    expect(holdPlacer(heldOn('t'), map)).toBe('Lucidos');
    expect(holdPlacer(heldOn('t', { kind: 'device' } as ThreadHoldSummary['placed_by']), map)).toBe('You');
    const fromAgent = { kind: 'api', source_thread_id: 'agent' } as ThreadHoldSummary['placed_by'];
    expect(holdPlacer(heldOn('t', fromAgent), map)).toBe('The agent of Ship the fix');
    const fromGone = { kind: 'api', source_thread_id: 'gone' } as ThreadHoldSummary['placed_by'];
    expect(holdPlacer(heldOn('t', fromGone), map)).toBe('An agent');
  });
});

describe('holdWaiting', () => {
  const root = makeThreadState('root', { meta: { hold: heldOn('root') } });
  const reached = makeThreadState('reached', { meta: { hold: heldOn('root') } });
  const other = makeThreadState('other', { meta: { hold: heldOn('other') } });
  const map = threads(root, reached, other);

  it('says nothing waits when the hold reached nothing', () => {
    expect(holdWaiting('root', threads(root), () => false)).toBe('Nothing else waits with it.');
  });

  it('counts the sub-threads the hold reached and their set-aside changes', () => {
    expect(holdWaiting('root', map, () => false)).toBe('1 sub-thread waits with it.');
    expect(holdWaiting('root', map, (id) => id === 'root' || id === 'reached')).toBe('1 sub-thread and 2 changes wait with it.');
  });
});

describe('HoldStrip', () => {
  let host: HTMLElement | null = null;
  afterEach(() => {
    if (host) render(null, host);
    host?.remove();
    host = null;
    vi.clearAllMocks();
  });

  function mount(focused: string, map: Map<string, ThreadState>): HTMLElement {
    threadMap.value = map;
    focusedThreadId.value = focused;
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<HoldStrip />, host);
    return host;
  }

  it('draws nothing for a thread that is not on hold', () => {
    const el = mount('t1', threads(makeThreadState('t1')));
    expect(el.querySelector('[data-role="hold-strip"]')).toBeNull();
  });

  it('shows the reason and offers Resume on the thread the hold was placed on', () => {
    const el = mount('root', threads(makeThreadState('root', { meta: { hold: heldOn('root') } })));
    expect(el.querySelector('.hold-strip-label')?.textContent).toBe('On hold: Waiting on the vendor');
    const resume = [...el.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Resume');
    resume!.click();
    expect(resumeHeldThread).toHaveBeenCalledWith('root');
  });

  it('links a reached sub-thread to the thread that holds Resume', () => {
    const root = makeThreadState('root', { meta: { title: 'Migrate billing', hold: heldOn('root') } });
    const child = makeThreadState('child', { meta: { hold: heldOn('root') } });
    const el = mount('child', threads(root, child));
    const buttons = [...el.querySelectorAll('button')].map((b) => b.textContent?.trim());
    expect(buttons).not.toContain('Resume');
    const open = [...el.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Open Migrate billing');
    open!.click();
    expect(focusThread).toHaveBeenCalledWith('root');
  });
});
