// @vitest-environment jsdom
/**
 * The pause strip and the People row draw only on a team gateway (ADR 0649).
 * On a personal gateway they read nothing and draw nothing, so it looks
 * exactly as before. On a team they read the engine and draw what it says.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (original) => ({
  ...(await original<typeof import('../../../api/client')>()),
  readAgentPause: vi.fn(async () => true),
  readThreadPeople: vi.fn(async () => ({
    root_thread_id: 'thread-1',
    driver: { id: 'sam', name: 'Sam' },
    watchers: [{ id: 'kim', name: 'Kim' }],
  })),
}));

import { readAgentPause, readThreadPeople } from '../../../api/client';
import { AgentPauseStrip } from '../AgentPauseStrip';
import { ThreadPeopleRow } from '../ThreadPeopleRow';
import { people } from '../../../store/actions/people';
import { focusedThreadId, threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

const THREAD = 'thread-1';
let host: HTMLDivElement;

async function draw(): Promise<void> {
  await act(async () => {
    render(<><ThreadPeopleRow threadId={THREAD} /><AgentPauseStrip /></>, host);
  });
  await act(async () => { await Promise.resolve(); });
}

beforeEach(() => {
  threadMap.value = new Map([[THREAD, makeThreadState(THREAD, { meta: { state: 'active' } })]]);
  focusedThreadId.value = THREAD;
  host = document.createElement('div');
  document.body.appendChild(host);
  vi.mocked(readAgentPause).mockClear();
  vi.mocked(readThreadPeople).mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
  people.value = { status: 'not-loaded' };
  threadMap.value = new Map();
  focusedThreadId.value = null;
});

describe('on a personal gateway', () => {
  it('reads nothing and draws neither control', async () => {
    people.value = { status: 'loaded', data: { people: [{ id: 'me', name: 'Me', has_device: true }], you: 'me', team: false } };
    await draw();
    expect(readAgentPause).not.toHaveBeenCalled();
    expect(readThreadPeople).not.toHaveBeenCalled();
    expect(host.querySelector('[data-role="thread-people-row"]')).toBeNull();
    expect(host.querySelector('[data-role="agent-pause-strip"]')).toBeNull();
  });
});

describe('on a team gateway', () => {
  it('draws who drives and watches, and the paused strip with Resume', async () => {
    people.value = {
      status: 'loaded',
      data: { people: [{ id: 'sam', name: 'Sam', has_device: true }, { id: 'kim', name: 'Kim', has_device: true }], you: 'kim', team: true },
    };
    await draw();
    const row = host.querySelector('[data-role="thread-people-row"]');
    expect(row?.textContent).toContain('Sam drives');
    expect([...row!.querySelectorAll('button')].map((b) => b.textContent)).toEqual(['Unwatch', 'Take']);
    const strip = host.querySelector('[data-role="agent-pause-strip"]');
    expect(strip?.textContent).toContain('Agent paused');
    expect(strip?.querySelector('button')?.textContent).toBe('Resume');
  });
});
