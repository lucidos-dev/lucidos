// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChatExchange } from '../ChatExchange';
import { triggerFiredBody } from '../chat-exchange-parts';
import { computeExchanges, groupIntoExchanges, triggerRunInterruption } from '../../../store/thread-events';
import type { StoredEvent } from '../../../store/thread-events';
import type { FireRetry, RetrySkipReason } from '../../../generated/thread-event-wire';
import { makeThreadState } from '../../../store/__tests__/thread-events-helpers';
import * as threads from '../../../store/actions/threads';

const started: StoredEvent = {
  type: 'TriggerStarted', trigger_id: 'trigger', trigger_name: 'Release check',
  _eventId: 'fire', created: '2026-01-01T12:00:00Z',
};
const aborted: StoredEvent = {
  type: 'ResponseAborted', cause: 'engine_shutdown',
  request_event_id: 'fire', created: '2026-01-01T12:00:01Z',
};
const queued: FireRetry = { kind: 'queued', entry_id: 'entry', deadline: '2026-01-02T12:00:00Z' };
function interrupted(retry: FireRetry = queued): StoredEvent {
  return {
    type: 'TriggerRunInterrupted', trigger_id: 'trigger', trigger_name: 'Release check',
    fire_event_id: 'fire', retry, created: '2026-01-01T12:00:02Z',
  };
}
function history(...events: StoredEvent[]) {
  return new Map(events.map((event, i) => [i + 1, event]));
}

const hosts: HTMLDivElement[] = [];
afterEach(() => {
  for (const host of hosts.splice(0)) {
    act(() => render(null, host));
    host.remove();
  }
  vi.restoreAllMocks();
});
function host() {
  const node = document.createElement('div');
  document.body.appendChild(node);
  hosts.push(node);
  return node;
}
function mountNotice(retry: FireRetry) {
  const exchanges = groupIntoExchanges(history(started, aborted, interrupted(retry)));
  const node = host();
  act(() => render(<ChatExchange
    exchange={exchanges[1]} revision={0} streamingBuffer="" isLast
    threadId="cut-thread" threadIsCC={false} sessionCodingAgent="claude-code" threadIdle
    threadAwaitingAnswer={false} threadCanceling={false}
  />, node));
  return node;
}

describe('interrupted trigger runs', () => {
  it('groups the report under the abort even when it carries the fire request id', () => {
    const events = history(started, aborted, { ...interrupted(), request_event_id: 'fire' });
    const exchanges = groupIntoExchanges(events);
    expect(exchanges).toHaveLength(2);
    expect(triggerRunInterruption(exchanges[0])).toBeUndefined();
    expect(triggerRunInterruption(exchanges[1])?.retry).toEqual(queued);
  });

  it('keeps a late report with its abort when a newer message exists', () => {
    const events = history(started, aborted, { type: 'MessageReceived', text: 'What happened?' }, interrupted());
    const thread = makeThreadState(events);
    const before = computeExchanges(thread);
    const revision = before[1].revision ?? 0;
    events.set(5, { type: 'MessageReceived', text: 'Continue checking.' });
    const exchanges = computeExchanges(thread);
    expect(triggerRunInterruption(exchanges[1])?.fire_event_id).toBe('fire');
    expect(triggerRunInterruption(exchanges[2])).toBeUndefined();
    expect(exchanges[1].revision ?? 0).toBe(revision);
    expect(exchanges.map(({ revision: _, ...exchange }) => exchange))
      .toEqual(groupIntoExchanges(new Map(events)).map(({ revision: _, ...exchange }) => exchange));
  });

  it('increments the abort revision when a report arrives through SSE', () => {
    const thread = makeThreadState(history(started, aborted));
    const before = computeExchanges(thread)[1].revision ?? 0;
    thread.events.set(3, interrupted());
    const exchange = computeExchanges(thread)[1];
    expect(triggerRunInterruption(exchange)?.retry.kind).toBe('queued');
    expect(exchange.revision).toBeGreaterThan(before);
  });

  it('retains a report when the abort is outside the loaded page', () => {
    const exchanges = groupIntoExchanges(history(interrupted()), true);
    expect(exchanges).toHaveLength(1);
    expect(triggerRunInterruption(exchanges[0])?.retry).toEqual(queued);
  });

  it('renders the queued decision beneath an engine-shutdown abort without an agent response', () => {
    const node = mountNotice(queued);
    const notice = node.querySelector('[data-role="trigger-interrupted"]');
    expect(notice?.textContent).toContain('An engine restart interrupted this run');
    expect(notice?.textContent).toContain('Running it again once');
    expect(node.querySelector('.response-panel')).toBeNull();
    expect(node.querySelector('.initiator-panel')?.compareDocumentPosition(notice!))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });

  const skipped: Record<RetrySkipReason, string> = {
    already_retried: 'This run was already a retry.',
    fire_completed: 'Its main work had finished. Use Continue to resume the rest.',
    spawned_work: 'Its sub-threads carry on by themselves.',
    too_old: 'It started more than 24 hours ago.',
    later_slot_due: 'Its next scheduled run is already due.',
    newer_fire: 'A newer run of this trigger has started.',
    trigger_unavailable: 'The trigger is paused, disabled or deleted.',
    request_unavailable: 'Lucidos could not find what started it.',
    already_queued: 'Another run of this trigger is already queued.',
  };
  for (const [reason, sentence] of Object.entries(skipped)) {
    it(`explains a skipped retry: ${reason}`, () => {
      const node = mountNotice({ kind: 'skipped', reason: reason as RetrySkipReason });
      const notice = node.querySelector('[data-role="trigger-interrupted"]');
      expect(notice?.textContent).toContain(`It was not run again. ${sentence}`);
      expect(notice?.textContent).not.toContain('Running it again once');
    });
  }

  it('links a retry header to the interrupted thread through the existing thread opener', () => {
    const open = vi.spyOn(threads, 'focusThreadOrBootstrap').mockReturnValue(undefined);
    const node = host();
    act(() => render(triggerFiredBody({
      event: { ...started, retry_of: { thread_id: 'cut-thread', deadline: queued.deadline } } as Extract<StoredEvent, { type: 'TriggerStarted' }>,
      opening: false,
    }), node));
    const link = node.querySelector<HTMLButtonElement>('[data-thread-id="cut-thread"]');
    expect(link?.textContent).toBe('Retry of an interrupted run');
    act(() => link!.click());
    expect(open).toHaveBeenCalledWith('cut-thread');
  });

  it('keeps a legacy trigger header free of retry context', () => {
    const node = host();
    act(() => render(triggerFiredBody({ event: started as Extract<StoredEvent, { type: 'TriggerStarted' }>, opening: false }), node));
    expect(node.textContent).not.toContain('Retry');
    expect(node.querySelector('[data-thread-id]')).toBeNull();
  });
});
