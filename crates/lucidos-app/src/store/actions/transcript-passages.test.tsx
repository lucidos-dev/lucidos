// @vitest-environment jsdom
/** Parity: every passage thread find counts is the text its element draws.
 *  Each turn renders through the real `ChatExchange`, so a card or a step row
 *  whose wording drifts from its passage fails here. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { collectPageText } from '@lucidos/find';
import { ChatExchange } from '../../components/chat/ChatExchange';
import { SideQuestionCard } from '../../components/chat/SideQuestionCard';
import { makeThreadState } from '../__tests__/thread-events-helpers';
import { detailsExpanded, openSubAgentGroups, stepsExpanded, threadMap } from '../store';
import { computeExchanges } from '../thread-events/exchange-grouping';
import { exchangeKey } from '../thread-events/exchange';
import type { SideQuestion } from '../sideQuestions';
import type { StoredEvent, ThreadEvent } from '../thread-events';
import { freshText, transcriptPassages, type TranscriptPassage } from './transcript-passages';

const ID = 'thread-1';
const at = (s: number) => `2026-10-06T10:00:${String(s).padStart(2, '0')}Z`;

const events: ThreadEvent[] = [
  { type: 'MessageReceived', text: 'Review the **plan**' } as ThreadEvent,
  { type: 'CodingAgentPromptSent' } as ThreadEvent,
  { type: 'CodingAgentTextStreamed', text: 'Reading it first.' } as ThreadEvent,
  { type: 'CodingAgentToolCalled', name: 'Read', args: { file_path: '/repo/plan.md' }, tool_use_id: 't1' } as ThreadEvent,
  { type: 'CodingAgentToolResult', name: '', result: 'ok', tool_use_id: 't1' } as ThreadEvent,
  { type: 'CodingAgentToolCalled', name: 'Agent', args: { description: 'Plan review' }, tool_use_id: 'ag' } as ThreadEvent,
  { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'approve' }, tool_use_id: 'c1', parent_tool_use_id: 'ag' } as ThreadEvent,
  { type: 'CodingAgentToolResult', name: '', result: 'ok', tool_use_id: 'c1', parent_tool_use_id: 'ag' } as ThreadEvent,
  { type: 'CodingAgentToolResult', name: '', result: 'done', tool_use_id: 'ag' } as ThreadEvent,
  { type: 'CodingAgentTextStreamed', text: 'The plan reads well.' } as ThreadEvent,
  { type: 'CommandCheckpointed', checkpoint_id: 'cp1', command: 'rm -rf build', summary: 'Removes the build output.' } as ThreadEvent,
  { type: 'SpokenReplyGenerated', text: 'The plan is sound.', interrupted: true } as ThreadEvent,
  { type: 'EventWaitStarted', wait_id: 'w1', tool_use_id: 'aw', watermark: 0, on: [], reason: 'the plan review', expires_at: '2026-10-06T11:00:00Z' } as ThreadEvent,
  { type: 'ResponseFailed', error: 'The model ran out of plan.' } as ThreadEvent,
  {
    type: 'UserQuestionAsked', tool_use_id: 'q1', cc_session_id: '', question: 'Which **plan** wins?', multi_select: false,
    options: [
      { id: 'a', label: 'Implement here', description: 'Approves the *plan*.' },
      { id: 'b', label: 'Fresh session' },
    ],
  } as ThreadEvent,
  { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'FreeText', text: 'Do *both*', image_hashes: [] } } as ThreadEvent,
  {
    type: 'CodingAgentPermissionRequest', request_id: 'r1', tool_use_id: 'tu1', tool_name: 'Bash',
    input: { command: 'git status', description: 'Show the working tree' }, summary: 'Bash git status',
  } as ThreadEvent,
  { type: 'CodingAgentPermissionResolved', request_id: 'r1', allowed: false, reason: 'Interrupted by an engine restart' } as ThreadEvent,
  {
    type: 'CommandPermissionRequested', request_id: 'r2', tool_use_id: 'tu2', tool_name: 'run_bash',
    command: 'ls -la', summary: 'Lists files.',
  } as ThreadEvent,
  {
    type: 'McpPermissionRequested', request_id: 'r3', tool_use_id: 'tu3', server_id: 's', server_name: 'docs',
    tool_name: 'search', arguments_summary: '{"q":"plan"}',
  } as ThreadEvent,
  {
    type: 'UserQuestionAsked', tool_use_id: 'q2', cc_session_id: '', question: 'Ship it?', multi_select: false,
    options: [{ id: 'y', label: 'Yes' }],
  } as ThreadEvent,
  { type: 'UserQuestionAnswered', tool_use_id: 'q2', answer: { kind: 'Superseded' } } as ThreadEvent,
];

const sideQuestion: SideQuestion = {
  id: 'sq1', threadId: ID, question: 'Why the *plan*?', imageHashes: [], afterSeq: 3, dismissed: false, asks: 1,
  status: 'answered', answer: 'Because it **approves**.',
};

let host: HTMLDivElement;

function threadState(list: ThreadEvent[] = events) {
  const map = new Map<number, StoredEvent>(list.map((e, i) => [i + 1, { ...e, created: at(i), _eventId: `e${i + 1}` } as StoredEvent]));
  const state = makeThreadState(map);
  threadMap.value = new Map([[ID, state]]);
  return state;
}

function drawThread(sideQuestions: SideQuestion[] = [sideQuestion]) {
  const state = threadMap.value.get(ID)!;
  act(() => {
    render(
      <div class="thread-feed">
        {computeExchanges(state).map((exchange) => (
          <ChatExchange
            key={exchangeKey(exchange)}
            exchange={exchange}
            revision={0}
            streamingBuffer=""
            isLast={false}
            threadId={ID}
            threadIsCC={true}
            sessionCodingAgent="claude-code"
            threadIdle={true}
            threadAwaitingAnswer={false}
            threadCanceling={false}
          />
        ))}
        {sideQuestions.map((item) => <SideQuestionCard key={item.id} item={item} />)}
      </div>,
      host,
    );
  });
}

function drawnText(passage: TranscriptPassage): string | null {
  const container = passage.where.inFeed
    ? host.querySelector('.thread-feed')!
    : host.querySelector(`.chat-exchange[data-user-seq="${passage.userSeq}"]`);
  const roots = [...(container?.querySelectorAll(passage.where.selector) ?? [])];
  const root = roots[passage.where.nth ?? 0];
  return root ? collectPageText(root).text : null;
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  stepsExpanded.value = true;
  detailsExpanded.value = true;
  openSubAgentGroups.value = new Set(['ag']);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  threadMap.value = new Map();
});

describe('thread find passages', () => {
  it('count each card, step row and side question, as drawn', () => {
    const passages = transcriptPassages(threadState(), freshText, [sideQuestion]);
    drawThread();
    expect(passages.length).toBeGreaterThan(0);
    for (const passage of passages) {
      expect(drawnText(passage), passage.where.selector).toBe(passage.text);
    }
  });

  it('count the turn-limit card, as drawn', () => {
    const passages = transcriptPassages(threadState([
      { type: 'MessageReceived', text: 'Go on' } as ThreadEvent,
      { type: 'ResponseGenerated', text: '[ENGINE-LIMIT] Per-turn limit of 50 tool calls reached.' } as ThreadEvent,
    ]), freshText);
    drawThread([]);
    const limit = passages.find((p) => p.where.selector.startsWith('.exchange-engine-limit'));
    expect(limit?.text).toContain('Per-turn limit');
    expect(drawnText(limit!)).toBe(limit!.text);
  });

  it('count the reason on a coding agent exit card, as drawn', () => {
    const passages = transcriptPassages(threadState([
      { type: 'MessageReceived', text: 'Go on' } as ThreadEvent,
      { type: 'ResponseFailed', error: 'segfault', agent_exit: { stage: 'mid_turn', code: 139 } } as ThreadEvent,
    ]), freshText);
    drawThread([]);
    const reason = passages.find((p) => p.where.selector === '.exchange-error-reason');
    expect(reason?.text).toContain('exited with code 139');
    expect(drawnText(reason!)).toBe(reason!.text);
  });

  it('reach every kind the transcript draws', () => {
    const text = transcriptPassages(threadState(), freshText, [sideQuestion])
      .map((p) => p.text.replace(/\u0000/g, ' ').trim());
    expect(text).toEqual(expect.arrayContaining([
      'Which plan wins?',
      'Implement here',
      'Approves the plan.',
      'Do both',
      'Replaced by your next message',
      'Closed when Lucidos restarted. Nobody denied it.',
      'Why the plan?',
      'Because it approves.',
    ]));
    const joined = text.join('\n');
    for (const fragment of ['Removes the build output.', 'rm -rf build', 'The plan is sound.', 'The plan review', 'The model ran out of plan.', 'git status', 'Show the working tree', 'ls -la', 'Lists files.', '{"q":"plan"}', 'search', 'Plan review']) {
      expect(joined).toContain(fragment);
    }
  });

  it('say what hides them', () => {
    const passages = transcriptPassages(threadState(), freshText, [{ ...sideQuestion, dismissed: true }]);
    const unfoldOf = (text: string) => passages.find((p) => p.text.replace(/\u0000/g, ' ').includes(text))?.unfold;
    expect(unfoldOf('Approves the plan.')).toEqual({ initiator: true });
    expect(unfoldOf('Show the working tree')).toEqual({ initiator: true });
    expect(unfoldOf('Plan review')).toMatchObject({ steps: true });
    expect(unfoldOf('Plan review')).not.toHaveProperty('subAgent');
    expect(unfoldOf('approve')).toMatchObject({ steps: true, subAgent: 'ag' });
    expect(unfoldOf('Because it approves.')).toEqual({ sideQuestion: 'sq1' });
  });
});
