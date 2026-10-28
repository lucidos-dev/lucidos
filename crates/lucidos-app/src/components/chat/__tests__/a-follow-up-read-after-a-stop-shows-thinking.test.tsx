// @vitest-environment jsdom
/** A follow-up read once the reader canceled a question card shows a Thinking
 *  row while the agent works on it. Reported: the transcript showed nothing
 *  under the follow-up, and only the title spinner said the agent was working.
 *  The fold half is pinned in
 *  `store/__tests__/an-unread-coding-agent-message-waits-in-the-queue.test.ts`.
 */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import { computeExchanges, type StoredEvent, type ThreadState } from '../../../store/thread-events';

const TS = '2026-10-28T07:15:00Z';

function runningThread(events: StoredEvent[]): ThreadState {
  return {
    meta: {
      id: 't1',
      title: 'Follow-up after a stop',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: TS,
      updatedAt: TS,
      status: 'running',
      summaryVersion: 0,
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      codingAgent: 'claude-code',
    },
    events: new Map(events.map((event, i) => [i + 1, event])),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: events.length,
    pendingUserMessages: [],
  };
}

const cc = { channel: 'claude_code', created: TS };
const events = [
  { type: 'MessageReceived', text: 'start', _eventId: 'm1' },
  { type: 'CodingAgentInputRead', input_event_id: 'm1', started_turn: true },
  { type: 'CodingAgentTextStreamed', text: 'working on it' },
  { type: 'MessageReceived', text: 'relay or pair?', _eventId: 'm2' },
  { type: 'CodingAgentPromptSent', text: 'relay or pair?', input_event_id: 'm2' },
  { type: 'UserQuestionAsked', tool_use_id: 'q1#q0', question: 'Which origin?', options: [{ id: 'opt-0', label: 'Document the trust' }], _eventId: 'q1' },
  { type: 'UserQuestionAnswered', tool_use_id: 'q1#q0', answer: { kind: 'Canceled' } },
  { type: 'CodingAgentToolResult', name: 'AskUserQuestion', result: 'rejected', tool_use_id: 'q1' },
  { type: 'ResponseCanceled', cause: 'user_stop' },
  { type: 'ProposalWithheld', reason: 'turn_incomplete', files: ['src/a.rs'], branch_name: 'b' },
  { type: 'CodingAgentIdled', has_changes: true },
  { type: 'CodingAgentSettingsChanged', model: 'opus' },
  { type: 'CodingAgentInputRead', input_event_id: 'm2', started_turn: true },
].map(e => ({ ...cc, ...e }) as StoredEvent);

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

it('draws a Thinking row under the follow-up while the agent works on it', () => {
  const thread = runningThread(events);
  threadMap.value = new Map([['t1', thread]]);
  act(() => { render(<div>{renderExchanges(computeExchanges(thread), 't1', '')}</div>, host); });
  const turns = [...host.querySelectorAll('.chat-exchange')];
  const followUp = turns[turns.length - 1];
  expect(followUp.textContent).toContain('relay or pair?');
  expect(followUp.querySelector('[data-thinking-row]')).not.toBeNull();
});
