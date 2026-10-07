import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeResponseText, exchangeSteps, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';
import type { ResponseEvent } from '../types';
import { rowsDrawnByClamp } from '../event-rendering';

type StepEvent = Extract<ResponseEvent, { type: 'step' }>;

const at = (s: number) => `2026-10-06T10:00:${String(s).padStart(2, '0')}Z`;

function call(id: string, name: string, args: Record<string, unknown>, parent?: string, apiCall?: string): ThreadEvent {
  return {
    type: 'CodingAgentToolCalled',
    name,
    args,
    tool_use_id: id,
    ...(parent ? { parent_tool_use_id: parent } : {}),
    ...(apiCall ? { api_call_id: apiCall } : {}),
  } as ThreadEvent;
}

function result(id: string, parent?: string): ThreadEvent {
  return {
    type: 'CodingAgentToolResult',
    name: '',
    result: 'ok',
    tool_use_id: id,
    ...(parent ? { parent_tool_use_id: parent } : {}),
  } as ThreadEvent;
}

function capture(tokens: number, parent?: string, apiCall?: string): ThreadEvent {
  return {
    type: 'ContextCaptured',
    producer: 'claude_code',
    model: 'claude-sonnet-5',
    context_window: 1_000_000,
    sections: [],
    estimated_total_tokens: tokens,
    ...(parent ? { parent_tool_use_id: parent } : {}),
    ...(apiCall ? { api_call_id: apiCall } : {}),
  } as ThreadEvent;
}

function exchangeOf(events: ThreadEvent[]) {
  const map = new Map<number, StoredEvent>(
    [{ type: 'MessageReceived', text: 'review it' } as ThreadEvent, ...events]
      .map((e, i): [number, StoredEvent] => [i + 1, { ...e, created: at(i) }]),
  );
  return groupIntoExchanges(map)[0];
}

const steps = (events: ResponseEvent[]) => events.filter((e): e is StepEvent => e.type === 'step');
const labels = (list: StepEvent[] | undefined) => (list ?? []).map(s => s.tool_use_id);

/** Two agents running in parallel, their steps interleaved with each other and
 *  with a parent capture, as Claude Code streams them. Each capture lands while
 *  another agent's step is the last one, which is what bound it wrongly. */
const twoParallelAgents: ThreadEvent[] = [
  { type: 'CodingAgentPromptSent' } as ThreadEvent,
  call('agent-a', 'Agent', { description: 'Correctness review' }),
  call('agent-b', 'Agent', { description: 'Simplification review' }),
  call('a1', 'Bash', { command: 'git diff --stat' }, 'agent-a'),
  call('b1', 'Grep', { pattern: 'fold' }, 'agent-b'),
  capture(42_000, 'agent-a'),
  capture(50_000, 'agent-b'),
  capture(653_000),
  result('a1', 'agent-a'),
  call('a2', 'Read', { file_path: 'fold.rs' }, 'agent-a'),
  result('b1', 'agent-b'),
  capture(44_000, 'agent-a'),
];

describe('sub-agent steps fold under their agent row', () => {
  it('nests each sub-agent step under the Agent call that spawned it, in clock order', () => {
    const top = steps(exchangeResponseEvents(exchangeOf(twoParallelAgents)));

    expect(labels(top)).toEqual(['agent-a', 'agent-b']);
    expect(labels(top[0].children)).toEqual(['a1', 'a2']);
    expect(labels(top[1].children)).toEqual(['b1']);
  });

  it('keeps a parent row in clock order around a group, never inside it', () => {
    const top = steps(exchangeResponseEvents(exchangeOf([
      call('agent-a', 'Agent', { description: 'Background review' }),
      call('a1', 'Bash', { command: 'ls' }, 'agent-a'),
      call('own', 'Read', { file_path: 'main.rs' }),
      call('a2', 'Bash', { command: 'pwd' }, 'agent-a'),
    ])));

    expect(labels(top)).toEqual(['agent-a', 'own']);
    expect(labels(top[0].children)).toEqual(['a1', 'a2']);
  });

  it('leaves a step whose parent is not in the turn top-level, as today', () => {
    const top = steps(exchangeResponseEvents(exchangeOf([
      call('orphan', 'Bash', { command: 'ls' }, 'agent-elsewhere'),
    ])));

    expect(labels(top)).toEqual(['orphan']);
  });

  it('binds each capture to the last step of its own agent', () => {
    const top = steps(exchangeResponseEvents(exchangeOf(twoParallelAgents)));
    const [agentA, agentB] = top;
    const tokens = (s: StepEvent | undefined) => s?.contextCapture?.estimated_total_tokens;

    expect(tokens(agentA.children?.[0])).toBe(42_000);
    expect(tokens(agentA.children?.[1])).toBe(44_000);
    expect(tokens(agentB.children?.[0])).toBe(50_000);
    // The parent's own capture stays on a parent row.
    expect(tokens(agentB)).toBe(653_000);
  });

  it('binds captures by agent in the flat step projection too', () => {
    const flat = exchangeSteps(exchangeOf(twoParallelAgents));
    const byId = (id: string) => flat.find(s => s.tool_use_id === id)?.contextCapture?.estimated_total_tokens;

    expect(byId('agent-b')).toBe(653_000);
    expect(byId('a1')).toBe(42_000);
    expect(byId('b1')).toBe(50_000);
    expect(byId('a2')).toBe(44_000);
  });

  it('never lets a sub-agent call name the parent’s pending Thinking row', () => {
    const exchange = exchangeOf([
      call('agent-a', 'Agent', { description: 'Background review' }),
      { type: 'CodingAgentThoughtStreamed', text: 'weighing it' } as ThreadEvent,
      call('a1', 'Bash', { command: 'ls' }, 'agent-a'),
    ]);

    const top = steps(exchangeResponseEvents(exchange));
    expect(top.map(s => s.description)).toEqual(['Background review', 'Thinking']);
    expect(labels(top[0].children)).toEqual(['a1']);

    const flat = exchangeSteps(exchange);
    expect(flat.some(s => s.description === 'Thinking' && s.outcome === 'pending')).toBe(true);
  });

  it('keeps a sub-agent’s narration out of the reply', () => {
    const exchange = exchangeOf([
      call('agent-a', 'Agent', { description: 'Review' }),
      { type: 'CodingAgentTextStreamed', text: 'I will read the files.', parent_tool_use_id: 'agent-a' } as ThreadEvent,
      call('a1', 'Bash', { command: 'ls' }, 'agent-a'),
      { type: 'CodingAgentTextStreamed', text: 'Both reviews are in.' } as ThreadEvent,
    ]);

    const texts = exchangeResponseEvents(exchange).filter(e => e.type === 'text').map(e => e.md);
    expect(texts).toEqual(['Both reviews are in.']);
    expect(exchangeResponseText(exchange)).toBe('Both reviews are in.');
  });

  it('counts a group as one row in the render window', () => {
    const events = exchangeResponseEvents(exchangeOf(twoParallelAgents));
    const drawn = rowsDrawnByClamp(events, { showSteps: true, showDetails: true, folded: false });

    expect(drawn.filter(Boolean)).toHaveLength(2);
  });

  it('settles a still-running child when the turn ends', () => {
    const top = steps(exchangeResponseEvents(exchangeOf([
      call('agent-a', 'Agent', { description: 'Review' }),
      call('a1', 'Bash', { command: 'ls' }, 'agent-a'),
      { type: 'CodingAgentIdled' } as ThreadEvent,
    ])));

    expect(top[0].children?.[0].outcome).not.toBe('pending');
  });
});

/** One API call can make several tool calls. Its capture carries the call's id,
 *  and so does each tool call, which is how every row of the batch finds it. */
describe('a capture binds to every tool call of its API call', () => {
  const tokens = (s: { contextCapture?: { estimated_total_tokens: number } } | undefined) =>
    s?.contextCapture?.estimated_total_tokens;

  /** The session's own batch: its capture lands after the last call. */
  const sessionBatch: ThreadEvent[] = [
    { type: 'CodingAgentPromptSent' } as ThreadEvent,
    call('agent-a', 'Agent', { description: 'Bug detection' }, undefined, 'msg_own'),
    call('agent-b', 'Agent', { description: 'Compliance' }, undefined, 'msg_own'),
    call('agent-c', 'Agent', { description: 'Regression check' }, undefined, 'msg_own'),
    capture(438_000, undefined, 'msg_own'),
  ];

  /** A sub-agent's batch: its capture lands after the FIRST call. */
  const subAgentBatch: ThreadEvent[] = [
    call('agent-a', 'Agent', { description: 'Review' }, undefined, 'msg_own'),
    capture(400_000, undefined, 'msg_own'),
    call('a1', 'Read', { file_path: 'a.rs' }, 'agent-a', 'msg_sub_1'),
    capture(42_000, 'agent-a', 'msg_sub_1'),
    call('a2', 'Read', { file_path: 'b.rs' }, 'agent-a', 'msg_sub_1'),
    result('a1', 'agent-a'),
    result('a2', 'agent-a'),
    call('a3', 'Bash', { command: 'ls' }, 'agent-a', 'msg_sub_2'),
    capture(44_000, 'agent-a', 'msg_sub_2'),
  ];

  it('shows the session batch figure on every row', () => {
    const top = steps(exchangeResponseEvents(exchangeOf(sessionBatch)));
    expect(top.map(tokens)).toEqual([438_000, 438_000, 438_000]);

    const flat = exchangeSteps(exchangeOf(sessionBatch));
    expect(['agent-a', 'agent-b', 'agent-c'].map(id => tokens(flat.find(s => s.tool_use_id === id))))
      .toEqual([438_000, 438_000, 438_000]);
  });

  it('shows a sub-agent batch figure on a call that arrived after its capture', () => {
    const [agentA] = steps(exchangeResponseEvents(exchangeOf(subAgentBatch)));
    expect(tokens(agentA)).toBe(400_000);
    expect(agentA.children?.map(tokens)).toEqual([42_000, 42_000, 44_000]);

    const flat = exchangeSteps(exchangeOf(subAgentBatch));
    expect(['a1', 'a2', 'a3'].map(id => tokens(flat.find(s => s.tool_use_id === id))))
      .toEqual([42_000, 42_000, 44_000]);
  });

  it('keeps a text-only call’s figure on its Thinking row', () => {
    const exchange = exchangeOf([
      { type: 'CodingAgentPromptSent' } as ThreadEvent,
      { type: 'CodingAgentTextStreamed', text: 'Nothing to change.' } as ThreadEvent,
      capture(12_000, undefined, 'msg_text'),
    ]);
    const thinking = <S extends { description: string }>(list: S[]) => list.find(s => s.description === 'Thinking');

    expect(tokens(thinking(steps(exchangeResponseEvents(exchange))))).toBe(12_000);
    expect(tokens(thinking(exchangeSteps(exchange)))).toBe(12_000);
  });

  it('never moves a capture onto a row from another API call', () => {
    // A call that only answered in text made no tool call to carry its figure.
    const exchange = exchangeOf([...sessionBatch, capture(439_000, undefined, 'msg_text')]);

    expect(steps(exchangeResponseEvents(exchange)).map(tokens)).toEqual([438_000, 438_000, 438_000]);
    expect(exchangeSteps(exchange).filter(s => s.tool_use_id).map(tokens))
      .toEqual([438_000, 438_000, 438_000]);
  });
});
