// @vitest-environment jsdom
/**
 * The strip on top of the composer that offers a *successor thread* (plan
 * I19, I20). It shows only at a clean idle: nothing pending, no open card, no
 * running turn, no event wait and no queued message. After an Apply it sends
 * the typed request to a fresh thread and wakes nothing here. Past the nudge
 * it asks the agent to hand off.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', () => ({
  fetchThreadContext: vi.fn(),
  sendToSuccessor: vi.fn(),
}));
vi.mock('../../../store/actions/compose', () => ({
  sendFollowup: vi.fn(() => Promise.resolve()),
  updateCompose: vi.fn(),
}));
vi.mock('../../../store/actions/threads', () => ({ focusThreadOrBootstrap: vi.fn() }));

import { HANDOFF_REQUEST } from '@lucidos/engine-constants';
import { SuccessorStrip, contextSizeLabel } from '../SuccessorStrip';
import { fetchThreadContext, sendToSuccessor, type ThreadContext } from '../../../api/client';
import { sendFollowup, updateCompose } from '../../../store/actions/compose';
import { focusThreadOrBootstrap } from '../../../store/actions/threads';
import { focusedThreadId, successorOffer, threadMap } from '../../../store/store';
import { clearDraft } from '../../../store/composeDrafts';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import type { StoredEvent } from '../../../store/thread-events';

type MetaOverrides = NonNullable<NonNullable<Parameters<typeof makeThreadState>[1]>['meta']>;

const LARGE: ThreadContext = { input_tokens: 412_000, can_hand_off: true, offer: true, nudge: true, applied: true };
const OFFER_ONLY: ThreadContext = { input_tokens: 200_000, can_hand_off: true, offer: true, nudge: false, applied: true };
const SMALL: ThreadContext = { input_tokens: 40_000, can_hand_off: true, offer: false, nudge: false, applied: true };
const NEVER_APPLIED: ThreadContext = { ...LARGE, applied: false };

const capture = { type: 'ContextCaptured', channel: 'claude_code' } as StoredEvent;

function idleThread(meta: MetaOverrides = {}, events: StoredEvent[] = [capture]) {
  const thread = makeThreadState('t1', { meta: { channel: 'claude_code', section: 'inbox', ...meta } });
  events.forEach((event, i) => thread.events.set(i + 1, event));
  return thread;
}

const typed = { text: 'Add a dark-mode toggle', imageCount: 0 };
const empty = { text: '', imageCount: 0 };

describe('successorOffer', () => {
  it('offers to send a typed request after an Apply, over the offer', () => {
    expect(successorOffer(idleThread(), OFFER_ONLY, typed)).toBe('send');
  });

  it('offers to continue an idle thread with an empty composer, over the nudge', () => {
    expect(successorOffer(idleThread(), LARGE, empty)).toBe('continue');
  });

  it('offers nothing to a small thread', () => {
    expect(successorOffer(idleThread(), SMALL, typed)).toBeNull();
    expect(successorOffer(idleThread(), SMALL, empty)).toBeNull();
  });

  it('offers no send before any Apply landed', () => {
    expect(successorOffer(idleThread(), NEVER_APPLIED, typed)).toBeNull();
  });

  it('offers no continue under the nudge, and nothing with images attached', () => {
    expect(successorOffer(idleThread(), OFFER_ONLY, empty)).toBeNull();
    expect(successorOffer(idleThread(), LARGE, { text: 'look', imageCount: 1 })).toBeNull();
  });

  it('offers nothing until the context is read', () => {
    expect(successorOffer(idleThread(), null, typed)).toBeNull();
  });

  describe('blocks on each unclean state', () => {
    const blocked: [string, MetaOverrides][] = [
      ['a running turn', { status: 'running' }],
      ['an open question card', { status: 'waiting_for_user_answer' }],
      ['a failed session', { status: 'failed' }],
      ['an event wait', { liveEventWaitCount: 1 }],
      ['a pending change', { codingAgentChangeState: { kind: 'proposed', requires_restart: false } }],
      ['withheld work', { codingAgentChangeState: { kind: 'unproposed', reason: 'hardening_missing' } }],
      ['a thread that already handed off', { successor: { thread_id: 's1', title: 'Next' } }],
      ['an archived thread', { section: 'archived' }],
      ['a chat thread', { channel: 'chat' }],
    ];
    for (const [name, meta] of blocked) {
      it(name, () => {
        expect(successorOffer(idleThread(meta), LARGE, typed)).toBeNull();
        expect(successorOffer(idleThread(meta), LARGE, empty)).toBeNull();
      });
    }

    it('a message sent and not yet confirmed', () => {
      const thread = idleThread();
      thread.pendingUserMessages = [{ text: 'next', eventId: 'e1', created: '2026-01-01T00:00:01Z' }];
      expect(successorOffer(thread, LARGE, empty)).toBeNull();
    });

    it('a queued message the agent has not read', () => {
      const message = (id: string) => ({ type: 'MessageReceived', text: 'x', channel: 'claude_code', _eventId: id }) as StoredEvent;
      const read = (id: string) => ({ type: 'CodingAgentInputRead', input_event_id: id }) as StoredEvent;
      const thread = idleThread({}, [message('m1'), read('m1'), capture, message('m2')]);
      expect(successorOffer(thread, LARGE, empty)).toBeNull();
    });
  });
});

describe('SuccessorStrip', () => {
  beforeEach(() => {
    vi.mocked(fetchThreadContext).mockResolvedValue(LARGE);
    vi.mocked(sendToSuccessor).mockResolvedValue({ successor_thread_id: 's1', link: '[Next](thread:ws/s1)' });
  });

  const hosts: HTMLElement[] = [];

  afterEach(() => {
    // Clearing the body leaves a Preact tree subscribed, so unmount each one.
    for (const host of hosts.splice(0)) render(null, host);
    document.body.innerHTML = '';
    threadMap.value = new Map();
    focusedThreadId.value = null;
    clearDraft('t1');
    vi.clearAllMocks();
  });

  async function mount(meta: MetaOverrides): Promise<HTMLElement> {
    threadMap.value = new Map([['t1', idleThread(meta)]]);
    focusedThreadId.value = 't1';
    const host = document.createElement('div');
    document.body.appendChild(host);
    hosts.push(host);
    await act(async () => { render(<SuccessorStrip />, host); });
    await act(async () => { await Promise.resolve(); });
    return host;
  }

  it('sends the typed request to a successor and wakes nothing on this thread', async () => {
    const host = await mount({ composeText: 'Add a dark-mode toggle' });
    const button = host.querySelector<HTMLButtonElement>('.successor-strip button');
    expect(button?.textContent).toBe('Send to a new thread');
    expect(host.querySelector('.successor-strip-label')?.textContent).toBe('412k tokens here');
    expect(contextSizeLabel({ ...LARGE, input_tokens: 999_600 })).toBe('1M tokens here');
    await act(async () => { button!.click(); });
    expect(sendToSuccessor).toHaveBeenCalledWith('t1', 'Add a dark-mode toggle');
    expect(sendFollowup).not.toHaveBeenCalled();
    expect(updateCompose).toHaveBeenCalledWith('t1', { text: '', image_hashes: [] });
    expect(focusThreadOrBootstrap).toHaveBeenCalledWith('s1');
  });

  it('asks the agent to hand off from an empty composer', async () => {
    vi.mocked(fetchThreadContext).mockResolvedValue(NEVER_APPLIED);
    const host = await mount({});
    const button = host.querySelector<HTMLButtonElement>('.successor-strip button');
    expect(button?.textContent).toBe('Continue in a new thread');
    await act(async () => { button!.click(); });
    expect(sendFollowup).toHaveBeenCalledWith('t1', HANDOFF_REQUEST);
    expect(sendToSuccessor).not.toHaveBeenCalled();
  });

  it('reads no context while a turn runs, and draws nothing', async () => {
    const host = await mount({ status: 'running' });
    expect(fetchThreadContext).not.toHaveBeenCalled();
    expect(host.querySelector('.successor-strip')).toBeNull();
  });

  it('reads each thread once, and shows its reading again on a switch back', async () => {
    const host = await mount({});
    threadMap.value = new Map([...threadMap.value, ['t2', makeThreadState('t2', { meta: { channel: 'claude_code', section: 'inbox' } })]]);
    await act(async () => { focusedThreadId.value = 't2'; });
    await act(async () => { await Promise.resolve(); });
    await act(async () => { focusedThreadId.value = 't1'; });
    expect(host.querySelector('.successor-strip button')?.textContent).toBe('Continue in a new thread');
    expect(fetchThreadContext).toHaveBeenCalledTimes(2);
  });

  it('draws nothing when the context read fails', async () => {
    vi.mocked(fetchThreadContext).mockRejectedValue(new Error('engine down'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const host = await mount({ composeText: 'Add a dark-mode toggle' });
    expect(host.querySelector('.successor-strip')).toBeNull();
    warn.mockRestore();
  });
});
