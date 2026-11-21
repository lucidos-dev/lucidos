// @vitest-environment jsdom
/**
 * The warning on top of the composer of a thread that handed off to a
 * *legacy successor thread*. It waits for the user to start writing, moves the
 * message on request, and stays hidden after *Send here anyway* until the
 * composer is cleared.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/successor', () => ({
  latestSuccessor: vi.fn(() => 'new'),
  moveDraftToSuccessor: vi.fn(),
}));

import { HandedOffCard } from '../HandedOffCard';
import { moveDraftToSuccessor } from '../../../store/actions/successor';
import { focusedThreadId, threadMap } from '../../../store/store';
import { clearDraft, patchDraft } from '../../../store/composeDrafts';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

const hosts: HTMLElement[] = [];

async function mount(successor = true): Promise<HTMLElement> {
  const meta = { channel: 'claude_code' as const, ...(successor && { successor: { thread_id: 'new', title: 'Next' } }) };
  threadMap.value = new Map([['old', makeThreadState('old', { meta })]]);
  focusedThreadId.value = 'old';
  const host = document.createElement('div');
  document.body.appendChild(host);
  hosts.push(host);
  await act(async () => { render(<HandedOffCard />, host); });
  return host;
}

const card = (host: HTMLElement) => host.querySelector('[data-role="handed-off-card"]');
const option = (host: HTMLElement, label: string) =>
  host.querySelector<HTMLButtonElement>(`[aria-label="Choose: ${label}"]`)!;
const type = (text: string) => act(async () => { patchDraft('old', { text }); });

describe('HandedOffCard', () => {
  afterEach(() => {
    for (const host of hosts.splice(0)) render(null, host);
    document.body.innerHTML = '';
    threadMap.value = new Map();
    focusedThreadId.value = null;
    clearDraft('old');
    vi.clearAllMocks();
  });

  it('waits until the user writes in a thread that handed off', async () => {
    const host = await mount();
    expect(card(host)).toBeNull();
    await type('one more thing');
    expect(card(host)?.textContent).toContain('This thread already continued in Next');
  });

  it('never shows on a thread with no successor', async () => {
    const host = await mount(false);
    await type('one more thing');
    expect(card(host)).toBeNull();
  });

  it('moves the message on request', async () => {
    const host = await mount();
    await type('one more thing');
    await act(async () => { option(host, 'Move my message there').click(); });
    expect(moveDraftToSuccessor).toHaveBeenCalledWith('old');
  });

  it('stays shut after Send here anyway until the composer is cleared', async () => {
    const host = await mount();
    await type('one more thing');
    await act(async () => { option(host, 'Send here anyway').click(); });
    expect(card(host)).toBeNull();
    await type('one more thing, longer');
    expect(card(host)).toBeNull();
    await act(async () => { clearDraft('old'); });
    await type('a new message');
    expect(card(host)).not.toBeNull();
  });
});
