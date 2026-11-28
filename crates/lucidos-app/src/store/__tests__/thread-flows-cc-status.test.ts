import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getExchangesWithPending, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseText, exchangeStatus, exchangeTimestamp, exchangeUserMessage, getCodingAgentWaitingInfo } from '../thread-events';
import { isActive } from '../exchange-status';
import { displaySection } from '../../generated/thread-lifecycle';

beforeEach(resetSeqCounter);

describe('Bug: aborted Claude Code session (engine restart) should be in inbox for review', () => {
  it('ResponseAborted without pending changes → failed (red triangle indicates interruption)', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file contents', created: t(-109000) },
      { type: 'ResponseAborted', created: t(-100000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('failed');
  });

  it('ResponseAborted + inbox stored section → displaySection is current', () => {
    expect(displaySection('inbox', 'failed', false, false, false, false, false)).toBe('current');
  });

  it('aborted then recovered → running while recovery CC works', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseAborted', created: t(-100000) },
      { type: 'ContinuationStarted', created: t(-50000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-49000) },
      { type: 'CodingAgentPromptSent', text: 'The engine restarted...', created: t(-48000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-40000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });
});

describe('CC thread with post-completion activity bumps status back to running', () => {

  it('CC tool call after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', created: t(-300000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-299000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-290000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-289000) },
      { type: 'ResponseGenerated', created: t(-280000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-279000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-270000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-269000) },
      { type: 'SessionEnded', created: t(-268000) },
      { type: 'MessageReceived', text: 'Now do this', created: t(-60000) },
      { type: 'SessionStarted', session_id: 's2', created: t(-59000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: t(-50000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-49000) },
      { type: 'ResponseGenerated', created: t(-30000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('CC text stream after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'ResponseGenerated', created: t(-60000) },
      { type: 'CodingAgentTextStreamed', text: 'Working...', created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });

  it('CC thread where last event equals completion time → still goes to idleOrWaiting', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-60000) },
      { type: 'ChangeProposed', change_id: 'c1', created: t(-50000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-10000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('CC tool result after ResponseGenerated bumps status back to running', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix it', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, created: t(-30000) },
      { type: 'ResponseGenerated', created: t(-20000) },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok', created: t(-5000) },
    ]);

    expect(map.get(id)!.meta.status).toBe('running');
  });
});

describe('Bug: queued message must not inherit steps from active exchange', () => {
  it('steps arriving after pending message timestamp stay in the previous exchange', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Plan our summer trip', created: '2026-03-22T19:12:00.000Z' } as any,
      { type: 'ToolCalled', name: 'web_search', args: { query: 'tropical islands' }, created: '2026-03-22T19:12:30.000Z' } as any,
      { type: 'ToolResult', name: 'web_search', result: 'results...', created: '2026-03-22T19:12:31.000Z' } as any,
    ]);

    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Lag en fet floating navigasjon', eventId: 'msg-2', created: '2026-03-22T19:13:29.000Z' },
    ];

    insertEvents(map, id, [
      { type: 'ToolCalled', name: 'run_python', args: { code: '...' }, created: '2026-03-22T19:13:45.000Z' } as any,
      { type: 'ToolResult', name: 'run_python', result: 'done', created: '2026-03-22T19:13:50.000Z' } as any,
      { type: 'ToolCalled', name: 'run_browser', args: { script: '...' }, created: '2026-03-22T19:14:10.000Z' } as any,
      { type: 'ToolResult', name: 'run_browser', result: 'ok', created: '2026-03-22T19:14:20.000Z' } as any,
    ]);

    const exchanges = getExchangesWithPending(map, id, true);

    expect(exchanges).toHaveLength(2);
    expect(exchangeUserMessage(exchanges[0])).toBe('Plan our summer trip');
    expect(exchangeUserMessage(exchanges[1])).toBe('Lag en fet floating navigasjon');

    expect(exchanges[0].steps.length).toBe(6);
    expect(exchanges[1].steps.length).toBe(0);
  });

  it('pending message display timestamp is preserved even without created on synthetic event', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'First', created: '2026-03-22T19:12:00.000Z' } as any,
      { type: 'ResponseGenerated', created: '2026-03-22T19:12:30.000Z' } as any,
    ]);

    const thread = map.get(id)!;
    thread.pendingUserMessages = [
      { text: 'Second', eventId: 'msg-2', created: '2026-03-22T19:13:29.000Z' },
    ];

    const exchanges = getExchangesWithPending(map, id, true);
    expect(exchanges).toHaveLength(2);

    const ts = exchangeTimestamp(exchanges[1]);
    expect(ts).toBe('2026-03-22T19:13:29.000Z');
  });
});


describe('Bug: CC follow-up messages should never show "Queued" or premature "Working"', () => {
  it('CC follow-up with no steps should show "Requesting", not "Queued"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
    ]);

    map.get(id)!.pendingUserMessages.push({
      text: 'It\'s some safari thing no?',
      eventId: 'follow-1',
      created: '2026-03-24T16:25:21Z',
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    const status0 = exchangeStatus(exchanges[0], '', false, false, true);
    expect(status0).toBe('interrupted');

    const status1 = exchangeStatus(exchanges[1], '', true, true, true);
    expect(status1).not.toBe('queued');
    expect(status1).toBe('pending');
    expect(getLabel(exchanges[1], '', true, true, true)).toBe('Requesting');
  });

  it('last CC exchange with no steps should show "Requesting", not "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
    ]);

    map.get(id)!.pendingUserMessages.push({
      text: 'Had no issues with other browsers',
      eventId: 'follow-1',
      created: '2026-03-24T16:25:57Z',
    } as any);

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(2);

    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).not.toBe('coding-agent-working');
    expect(status).toBe('pending');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Requesting');
  });

  it('CC follow-up with CodingAgentPromptSent (no tool/text yet) → "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      { type: 'MessageReceived', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
      { type: 'CodingAgentPromptSent', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).toBe('coding-agent-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('CC follow-up WITH steps should still show "Working"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      { type: 'MessageReceived', text: 'now fix the tests', created: '2026-03-24T16:26:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-03-24T16:26:05Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);

    const status = exchangeStatus(exchanges[1], '', true, false, true);
    expect(status).toBe('coding-agent-working');
    expect(getLabel(exchanges[1], '', true, false, true)).toBe('Working');
  });

  it('non-last CC exchange with no steps should show "Done", not "Interrupted"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
      { type: 'ResponseGenerated', created: '2026-03-24T16:25:15Z' },
      { type: 'CodingAgentIdled', created: '2026-03-24T16:25:16Z' },
      { type: 'MessageReceived', text: 'first follow-up', created: '2026-03-24T16:26:00Z' },
      { type: 'MessageReceived', text: 'second follow-up', created: '2026-03-24T16:26:05Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-03-24T16:26:10Z' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);

    const status = exchangeStatus(exchanges[1], '', false, false, true);
    expect(status).toBe('done');
  });

  it('multiple CC follow-ups all pending — none should show "Queued"', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', channel: 'claude_code', created: '2026-03-24T16:25:00Z' },
      { type: 'SessionStarted', session_id: 's1', created: '2026-03-24T16:25:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-24T16:25:10Z' },
    ]);

    map.get(id)!.pendingUserMessages.push(
      { text: 'follow-up 1', eventId: 'f1', created: '2026-03-24T16:25:21Z' } as any,
      { text: 'follow-up 2', eventId: 'f2', created: '2026-03-24T16:25:57Z' } as any,
    );

    const exchanges = getExchangesWithPending(map, id);
    expect(exchanges).toHaveLength(3);

    for (let i = 1; i < exchanges.length; i++) {
      const isLast = i === exchanges.length - 1;
      const priorStatus = exchangeStatus(exchanges[i - 1], '', false, false, true);
      const hasPrior = isActive(priorStatus);
      const status = exchangeStatus(exchanges[i], '', isLast, hasPrior, true);
      expect(status).not.toBe('queued');
    }
  });
});

describe('CC idle session — getCodingAgentWaitingInfo state transitions', () => {
  it('cc waiting info is cleared when session ends', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 'claude-code/20260325' },
      { type: 'CodingAgentIdled', has_changes: true, cc_session_id: 'abc-123-session' },
      { type: 'SessionEnded' },
    ]);

    const info = getCodingAgentWaitingInfo(map.get(id)!.meta);
    expect(info).toBeNull();
  });

  it('cc waiting info is cleared when CodingAgentPromptSent arrives after idle', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'CodingAgentPromptSent', text: '/harden' },
    ]);

    const info = getCodingAgentWaitingInfo(map.get(id)!.meta);
    expect(info).toBeNull();
  });

  it('Discard & End Session: SessionEnded with reason=discarded, no ChangeProposed', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'implement feature', created: '2026-03-26T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-26T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-26T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-26T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-03-26T10:00:04Z' },
      { type: 'ChangeDiscarded', created: '2026-03-26T10:00:04.500Z' },
      { type: 'SessionEnded', reason: 'discarded', created: '2026-03-26T10:00:05Z' },
    ]);

    const thread = map.get(id)!;
    const events = [...thread.events.values()];

    expect(events.some(e => e.type === 'ChangeProposed')).toBe(false);

    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toBeNull();

    expect(thread.meta.status).toBe('idle');
  });

  it('Add to Changes: SessionEnded with ChangeProposed before it', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'implement feature', created: '2026-03-26T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-26T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-26T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-26T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-03-26T10:00:04Z' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Feature implementation', created: '2026-03-26T10:00:05Z' },
      { type: 'SessionEnded', reason: 'changes_proposed', created: '2026-03-26T10:00:06Z' },
    ]);

    const thread = map.get(id)!;
    const events = [...thread.events.values()];

    expect(events.some(e => e.type === 'ChangeProposed')).toBe(true);

    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toBeNull();

    expect(thread.meta.status).toBe('idle');
  });

  it('Discard without ChangeDiscarded: CodingAgentIdled { has_changes: false } clears stale flags', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor engine', created: '2026-03-31T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-31T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-31T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-31T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, requires_restart: true, created: '2026-03-31T10:00:04Z' },
      { type: 'CodingAgentIdled', has_changes: false, requires_restart: false, created: '2026-03-31T10:00:05Z' },
    ]);

    const thread = map.get(id)!;

    expect(thread.meta.codingAgentChangeState).toEqual({ kind: 'none' });
    expect(thread.meta.status).toBe('idle');
  });

  it('Stale discard without ChangeDiscarded: SessionEnded reason=discarded clears stale flags', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor engine', created: '2026-03-31T10:00:00Z' },
      { type: 'SessionStarted', session_id: 'cc-1', created: '2026-03-31T10:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: '2026-03-31T10:00:02Z' },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'done', created: '2026-03-31T10:00:03Z' },
      { type: 'CodingAgentIdled', has_changes: true, requires_restart: true, created: '2026-03-31T10:00:04Z' },
      { type: 'SessionEnded', reason: 'discarded', created: '2026-03-31T10:00:10Z' },
    ]);

    const thread = map.get(id)!;

    expect(thread.meta.codingAgentChangeState).toEqual({ kind: 'none' });
    expect(thread.meta.codingAgentIsExternalRepo).toBe(false);

    expect(thread.meta.status).toBe('idle');

    const info = getCodingAgentWaitingInfo(thread.meta);
    expect(info).toBeNull();
  });
});

describe('CC follow-up after resolved changes correctly shows running', () => {
  const now = new Date();
  const t = (offsetMs: number) => new Date(now.getTime() + offsetMs).toISOString();

  it('MessageReceived after all changes resolved → running', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug', created: t(-10000) },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: t(-9000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-8000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-7000) },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: t(-6000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      { type: 'MessageReceived', text: 'now fix the tests too', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');

    const section = displaySection('archived', 'running', false, false, false, false, false);
    expect(section).toBe('current');
  });

  it('CodingAgentUserMessageSent after resolved changes → running (with live process)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      { type: 'CodingAgentUserMessageSent', text: 'also fix tests', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });

  it('TriggerStarted after resolved changes → running (with live process)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'check logs', created: t(-10000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-9000) },
      { type: 'ResponseGenerated', created: t(-5000) },
      { type: 'ChangeProposed', change_id: 'c1', description: 'fix', files: ['a.ts'], requires_restart: false, created: t(-4000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-3000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-2000) },
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'daily check', created: t(-1000) },
    ]);

    const thread = map.get(id)!;
    expect(thread.meta.status).toBe('running');
  });
});

describe('CC follow-up abort: ResponseAborted is now an exchange boundary', () => {
  it('CC crash (no shutdown) opens an abort boundary exchange', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseGenerated', created: t(-105000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-100000) },
      { type: 'MessageReceived', text: 'Now fix tests', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
      { type: 'SessionEnded', created: t(-48000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[1], '', false, false, true)).toBe('aborted');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('shutdown abort still marks the original exchange aborted', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'ResponseAborted', created: t(-100000) },
      { type: 'SessionEnded', reason: 'shutdown', created: t(-99000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('aborted');
  });
});

describe('Aborted-exchange boundary: ResponseAborted opens its own panel', () => {
  it('CC follow-up aborted before any output: AbortPanel exchange is empty', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-109000) },
      { type: 'CodingAgentTextStreamed', text: 'Done fixing.', created: t(-108000) },
      { type: 'CodingAgentIdled', has_changes: true, created: t(-100000) },
      { type: 'ChangeApplied', change_id: 'c1', created: t(-95000) },
      { type: 'SessionEnded', reason: 'changes_applied', created: t(-94000) },
      { type: 'MessageReceived', text: 'The ios suite should have been included', channel: 'claude_code', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(4);
    expect(exchangeStatus(exchanges[0], '', false, false, true)).toBe('done');
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    expect(exchangeStatus(exchanges[2], '', false, false, true)).toBe('aborted');
    expect(exchanges[3].userEvent.type).toBe('ResponseAborted');
    expect(exchanges[3].steps).toHaveLength(0);
  });

  it('CC follow-up aborted AFTER producing output: prior exchange keeps its content', () => {
    const { map, id } = makeThread();
    map.get(id)!.meta.channel = 'claude_code';
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Fix the bug', channel: 'claude_code', created: t(-120000) },
      { type: 'SessionStarted', session_id: 's1', created: t(-119000) },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: {}, created: t(-110000) },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok', created: t(-109000) },
      { type: 'CodingAgentIdled', has_changes: false, created: t(-100000) },
      { type: 'MessageReceived', text: 'Now fix tests', channel: 'claude_code', created: t(-50000) },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, description: 'Reading test file', created: t(-48000) },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'ok', created: t(-47000) },
      { type: 'CodingAgentTextStreamed', text: 'Looking at the test failures...', created: t(-46000) },
      { type: 'ResponseAborted', created: t(-45000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    const followUp = exchanges[1];
    expect(exchangeStatus(followUp, '', false, false, true)).toBe('aborted');
    expect(exchangeResponseText(followUp)).toBe('Looking at the test failures...');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });

  it('chat exchange aborted: AbortPanel boundary opens after the original', () => {
    const { map, id } = makeThread();
    const now = Date.now();
    const t = (offset: number) => new Date(now + offset).toISOString();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello', channel: 'chat', created: t(-120000) },
      { type: 'TextStreamed', text: 'Hi there!', created: t(-119000) },
      { type: 'ResponseGenerated', created: t(-118000) },
      { type: 'MessageReceived', text: 'Now what?', channel: 'chat', created: t(-50000) },
      { type: 'ResponseAborted', created: t(-49000) },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[1], '', false)).toBe('aborted');
    expect(exchanges[2].userEvent.type).toBe('ResponseAborted');
  });
});
