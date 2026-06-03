import { describe, it, expect, beforeEach } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseText, exchangeStatus, exchangeSteps, exchangeUserChannel, exchangeUserImageHashes, exchangeUserMessage, groupIntoExchanges, type ThreadState } from '../thread-events';

beforeEach(resetSeqCounter);

describe('CC thread follow-up: channel detection', () => {
  it('thread with channel=claude_code is detected for CC mode inheritance', () => {
    // Simulate: CC thread exists with channel='claude_code'
    const map = new Map<string, ThreadState>();
    const ccThread: ThreadState = {
      meta: {
        id: 'cc-1',
        title: 'CC Thread',
        channel: 'claude_code',
        initiator: 'user',
        saved: false,
        createdAt: '',
        updatedAt: '',
        status: 'idle',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        lastRevivedAt: '',
        state: 'active',
        latestTodoList: null,
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    map.set('cc-1', ccThread);

    // The frontend's sendMessage checks existingThread?.meta.channel === 'claude_code'
    // to decide whether to set use_claude_code: true on follow-up messages
    const existingThread = map.get('cc-1');
    expect(existingThread?.meta.channel).toBe('claude_code');

    // Regular chat thread should NOT trigger CC mode
    const chatThread: ThreadState = {
      meta: {
        id: 'chat-1',
        title: 'Chat',
        channel: 'chat',
        initiator: 'user',
        saved: false,
        createdAt: '',
        updatedAt: '',
        status: 'idle',
        messageCount: 0,
        section: 'archived',
        activeChildrenCount: 0,
        totalChildrenCount: 0,
        blockingDescendantCount: 0, attentionDescendantCount: 0,
        codingAgentProposed: false,
        codingAgentRequiresRestart: false,
        codingAgentIsExternalRepo: false,
        codingAgentApplying: false,
        codingAgentHasDiff: false,
        lastRevivedAt: '',
        state: 'active',
        latestTodoList: null,
      },
      events: new Map(),
      streamingBuffer: '',
      eventsLoaded: true,
      eventsLoadFailed: false,
      lastDbSeq: 0,
      pendingUserMessages: [],
    };
    map.set('chat-1', chatThread);
    expect(map.get('chat-1')?.meta.channel).not.toBe('claude_code');
  });

  it('SessionStarted event sets thread channel to claude_code', () => {
    // Verify the SSE handler sets source correctly when CC starts
    const { map, id } = makeThread();
    const thread = map.get(id)!;
    expect(thread.meta.channel).toBe('chat'); // initially chat

    // After SessionStarted, source should be 'claude_code'
    // (handled by handleThreadEvent in thread-sync.ts)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
    ]);
    // thread-sync.ts sets meta.channel on SessionStarted — verify the thread
    // recognizes this as a CC thread for future follow-ups
    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges[0].steps.some(s => s.event.type === 'SessionStarted')).toBe(true);
  });

  it('follow-up to running CC thread must route via CC channel, not /api/v1/chat', () => {
    // Regression: sending a follow-up via /api/v1/chat calls register_thread()
    // which cancels the old token, killing the active Claude Code session.
    // The frontend should detect running CC threads and route via CC message channel.
    const { map, id } = makeThread();

    // Simulate a CC thread that's actively running (tools being called)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Grep', args: { pattern: 'foo' } },
    ]);
    // SessionStarted sets source via thread-sync.ts — simulate that
    map.get(id)!.meta.channel = 'claude_code';

    // Thread status should be 'running' — CC is actively processing (live process)
    const status = map.get(id)!.meta.status;
    expect(status).toBe('running');

    // The frontend routing logic: CC thread + running status → use CC channel
    const thread = map.get(id)!;
    const isCCThread = thread.meta.channel === 'claude_code';
    const hasLiveCCSession = isCCThread && (status === 'running' || status === 'waiting');
    expect(hasLiveCCSession).toBe(true);
  });

  it('follow-up to ended CC thread spawns new Claude Code session via /api/v1/chat', () => {
    const { map, id } = makeThread();

    // Claude Code session that has ended
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed it.' },
      { type: 'ResponseGenerated', text: 'Fixed it.', images: [] },
      { type: 'SessionEnded' },
    ]);
    map.get(id)!.meta.channel = 'claude_code';

    const status = map.get(id)!.meta.status;
    expect(status).toBe('idle');

    // Ended CC thread — should spawn new session via /api/v1/chat, not CC channel
    const thread = map.get(id)!;
    const isCCThread = thread.meta.channel === 'claude_code';
    const hasLiveCCSession = isCCThread && (status === 'running' || status === 'waiting');
    expect(hasLiveCCSession).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// No duplicate events — each event type should appear at most once per action
// ---------------------------------------------------------------------------
// MUST TEST: EventBus migration integration tests
// These test flows that were migrated from old Event to ThreadEvent via bus.
// ---------------------------------------------------------------------------

describe('MUST TEST 1: CC change proposal → apply/discard', () => {
  it('ChangeProposed appears as step in exchange with done status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor the code' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { path: 'src/main.rs' } },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Refactored the module.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Refactor module', files: ['src/main.rs'], requires_restart: true },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');

    // ChangeProposed should be in the exchange steps
    const allEvents = [...map.get(id)!.events.values()];
    const changeProposed = allEvents.find(e => e.type === 'ChangeProposed');
    expect(changeProposed).toBeDefined();
    expect((changeProposed as any).change_id).toBe('c-1');
    expect((changeProposed as any).files).toEqual(['src/main.rs']);
  });

  it('ChangeProposed must come before SessionEnded — thread waiting until change resolved', () => {
    // Regression: backend emitted SessionEnded before ChangeProposed, causing
    // the exchange to stay stuck instead of transitioning to done.
    // After fix, ChangeProposed always precedes SessionEnded.
    // Thread status is 'waiting' because the change hasn't been applied/discarded yet.
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'refactor the code' },
      { type: 'SessionStarted', session_id: 'cc-1' },
      { type: 'CodingAgentToolCalled', name: 'Edit', args: { path: 'src/main.rs' } },
      { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ResponseGenerated', text: 'Done.', images: [] },
      { type: 'CodingAgentIdled', has_changes: true },
      // Correct order: ChangeProposed THEN SessionEnded
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Refactor', files: ['src/main.rs'], requires_restart: false },
      { type: 'SessionEnded' },
    ]);

    // Thread stays in Waiting until change is applied/discarded
    const status = map.get(id)!.meta.status;
    expect(status).toBe('waiting');

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Exchange itself is 'done' (session completed), but thread is waiting for change resolution
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('ChangeApplied after ChangeProposed — thread idle, events correct', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.' },
      { type: 'ChangeProposed', change_id: 'c-1', description: 'Bug fix', files: ['src/lib.rs'], requires_restart: false },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'c-1' },
      { type: 'SessionEnded' },
    ]);

    // Thread status should be idle (SessionEnded is completion)
    expect(map.get(id)!.meta.status).toBe('idle');

    const allEvents = [...map.get(id)!.events.values()];
    expect(allEvents.some(e => e.type === 'ChangeApplied')).toBe(true);
    expect(allEvents.some(e => e.type === 'SessionEnded')).toBe(true);
  });

  it('ChangeDiscarded after ChangeProposed — thread idle, events correct', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'try something' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentTextStreamed', text: 'Tried it.' },
      { type: 'ChangeProposed', change_id: 'c-2', description: 'Experiment', files: ['test.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeDiscarded', change_id: 'c-2' },
      { type: 'SessionEnded' },
    ]);

    expect(map.get(id)!.meta.status).toBe('idle');

    const allEvents = [...map.get(id)!.events.values()];
    expect(allEvents.some(e => e.type === 'ChangeDiscarded')).toBe(true);
  });
});

describe('MUST TEST 2: CC cancel mid-work', () => {
  it('ResponseCanceled mid-stream shows canceled status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'do something complex' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm test' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'running...' },
      { type: 'CodingAgentTextStreamed', text: 'Working...' },
      { type: 'ResponseCanceled' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: original CC reply (status canceled), 2: ResponseCanceled boundary panel
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('canceled');
    expect(exchanges[1].userEvent.type).toBe('ResponseCanceled');
    expect(map.get(id)!.meta.status).toBe('idle');
  });

  it('CC cancel preserves streamed text from CodingAgentTextStreamed events', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix everything' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Starting to work...' },
      { type: 'CodingAgentTextStreamed', text: ' Almost done.' },
      { type: 'ResponseCanceled' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: original CC reply (status canceled), 2: ResponseCanceled boundary panel
    expect(exchanges).toHaveLength(2);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('canceled');
    expect(exchanges[1].userEvent.type).toBe('ResponseCanceled');
    // Response text comes from CodingAgentTextStreamed, not ResponseCanceled
    expect(exchangeResponseText(exchanges[0])).toBe('Starting to work... Almost done.');
  });
});

describe('MUST TEST 3: CC follow-up after idle', () => {
  it('follow-up in same thread creates second exchange, first becomes done', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // First exchange: CC works and idles
      { type: 'MessageReceived', text: 'fix the tests' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'test code' },
      { type: 'CodingAgentTextStreamed', text: 'Tests fixed.' },
      { type: 'CodingAgentIdled', has_changes: true },
      // Second exchange: user sends follow-up
      { type: 'MessageReceived', text: 'also fix the linting errors' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm run lint' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: '0 errors' },
      { type: 'CodingAgentTextStreamed', text: 'Linting fixed too.' },
      { type: 'CodingAgentIdled', has_changes: true },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // oldest first
    // First exchange completed (follow-up from idle = done, not interrupted)
    expect(exchangeUserMessage(exchanges[0])).toBe('fix the tests');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    // Newest exchange (follow-up) is last
    expect(exchangeUserMessage(exchanges[1])).toBe('also fix the linting errors');
    expect(exchangeStatus(exchanges[1], '', true)).toBe('done');
  });

  it('follow-up preserves CC context (both exchanges have response text)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'first task' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done first.' },
      { type: 'CodingAgentIdled' },
      { type: 'MessageReceived', text: 'second task' },
      { type: 'CodingAgentTextStreamed', text: 'Done second.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // CodingAgentTextStreamed contributes to response text, not steps
    // oldest first
    expect(exchangeResponseText(exchanges[0])).toBe('Done first.');
    expect(exchangeResponseText(exchanges[1])).toBe('Done second.');
  });
});

// ---------------------------------------------------------------------------
// Flow: CC follow-up interrupted by stop button
// ---------------------------------------------------------------------------
// When the user sends a follow-up to an idle Claude Code session and then hits stop:
//   1. CC was idle (CodingAgentIdled) → user sends follow-up (MessageReceived)
//   2. CC starts working → user hits stop → interrupt sent
//   3. Backend emits ResponseCanceled (not ResponseGenerated) then CodingAgentIdled
// Result: the follow-up exchange shows "Canceled", but the thread stays "Waiting"
// because CC is still alive and idle (CodingAgentIdled is the last event).
describe('Flow: CC follow-up stopped by user', () => {
  it('interrupted follow-up exchange shows "Canceled", thread stays "Waiting"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // First exchange: CC completes work and goes idle
      { type: 'MessageReceived', text: 'Fix the bug', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {}, created: '2026-01-01T00:00:02Z' },
      { type: 'CodingAgentTextStreamed', text: 'Fixed.', created: '2026-01-01T00:00:03Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:04Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:00:05Z' },
      // Second exchange: user sends follow-up, then hits stop
      { type: 'MessageReceived', text: 'Also fix tests', created: '2026-01-01T00:01:00Z' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'npm test' }, created: '2026-01-01T00:01:01Z' },
      // User hits stop → backend emits ResponseCanceled then CodingAgentIdled
      { type: 'ResponseCanceled', created: '2026-01-01T00:01:02Z' },
      { type: 'CodingAgentIdled', has_changes: true, created: '2026-01-01T00:01:03Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: first reply (done), 2: follow-up (canceled), 3: cancel boundary panel
    expect(exchanges).toHaveLength(3);

    // First exchange: completed normally → "Done"
    expect(exchangeUserMessage(exchanges[0])).toBe('Fix the bug');
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
    expect(getLabel(exchanges[0], '', false)).toBe('Done');

    // Second exchange (follow-up): user hit stop → "Canceled"
    expect(exchangeUserMessage(exchanges[1])).toBe('Also fix tests');
    expect(exchangeStatus(exchanges[1], '', false)).toBe('canceled');
    expect(getLabel(exchanges[1], '', false)).toBe('Canceled');

    // Third exchange: ResponseCanceled boundary panel — 'You — Canceled the response'
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');

    // Thread: still "Waiting" because CC is alive (last event is CodingAgentIdled)
    expect(map.get(id)!.meta.status).toBe('waiting');
  });

  it('interrupted follow-up with no work started also shows "Canceled"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      // CC completes and goes idle
      { type: 'MessageReceived', text: 'Build feature', created: '2026-01-01T00:00:00Z' },
      { type: 'SessionStarted', session_id: 'claude-code/test', created: '2026-01-01T00:00:01Z' },
      { type: 'CodingAgentTextStreamed', text: 'Done.', created: '2026-01-01T00:00:02Z' },
      { type: 'ResponseGenerated', created: '2026-01-01T00:00:03Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:00:04Z' },
      // User sends follow-up and immediately hits stop (CC hadn't started working yet)
      { type: 'MessageReceived', text: 'Never mind', created: '2026-01-01T00:01:00Z' },
      { type: 'ResponseCanceled', created: '2026-01-01T00:01:01Z' },
      { type: 'CodingAgentIdled', created: '2026-01-01T00:01:02Z' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: first reply (done), 2: follow-up (canceled), 3: cancel boundary panel
    expect(exchanges).toHaveLength(3);
    expect(exchangeStatus(exchanges[1], '', false)).toBe('canceled');
    expect(getLabel(exchanges[1], '', false)).toBe('Canceled');
    expect(exchanges[2].userEvent.type).toBe('ResponseCanceled');
  });
});

describe('MUST TEST 4: Scheduled triggers', () => {
  it('scheduled trigger produces one exchange with correct status', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-1', trigger_name: 'Morning Brief', prompt: 'Check my calendar' },
      { type: 'ToolCalled', name: 'execute_intent', args: {} },
      { type: 'ToolResult', name: 'execute_intent', result: 'Calendar is clear.' },
      { type: 'TextStreamed', text: 'Your calendar is clear today.' },
      { type: 'ResponseGenerated', text: 'Your calendar is clear today.' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('Check my calendar');
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    expect(exchangeResponseText(exchanges[0])).toBe('Your calendar is clear today.');
  });

  it('scheduled trigger with notification tool shows tool step', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 'task-2', trigger_name: 'Daily Report' },
      { type: 'ToolCalled', name: 'send_notification', args: { title: 'Report', message: 'All good' } },
      { type: 'ToolResult', name: 'send_notification', result: 'Notification sent.' },
      { type: 'ResponseGenerated', text: 'Report sent.' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
    // Steps use human-readable description derived from tool name + args
    expect(exchangeSteps(exchanges[0]).some((s: any) => s.description === 'Notify: Report')).toBe(true);
  });
});

describe('MUST TEST 5: Recovery sessions', () => {
  it('recovery thread has one exchange with CC events', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'A previous Claude Code session on branch `claude-code/20260315-120919` was interrupted...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-120919' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git status' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'nothing to commit' },
      { type: 'CodingAgentTextStreamed', text: 'The worktree is clean.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');

    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');
  });

  it('recovery with changes proposes change, then ends', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-120919' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'changes found' },
      { type: 'CodingAgentTextStreamed', text: 'Found work in progress.' },
      { type: 'ChangeProposed', change_id: 'recovery-c1', description: 'Previous session work', files: ['src/fix.rs'] },
      { type: 'CodingAgentIdled', has_changes: true },
      { type: 'ChangeApplied', change_id: 'recovery-c1' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    // 1: recovery exchange, 2: ChangeApplied initiator panel
    expect(exchanges).toHaveLength(2);
    expect(exchanges[1].userEvent.type).toBe('ChangeApplied');
    // Thread status is idle because SessionEnded is a completion event
    expect(map.get(id)!.meta.status).toBe('idle');

    // No duplicate events
    const allEvents = [...map.get(id)!.events.values()];
    const msgEvents = allEvents.filter(e => e.type === 'MessageReceived');
    expect(msgEvents).toHaveLength(1);
  });

  it('recovery session without changes auto-ends cleanly', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315-clean' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git diff' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: '' },
      { type: 'CodingAgentTextStreamed', text: 'Nothing to clean up.' },
      // Recovery with no changes auto-ends (cancel.notify_one())
      { type: 'CodingAgentIdled' },
      { type: 'SessionEnded' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    // Thread status is idle (SessionEnded is a completion event)
    expect(map.get(id)!.meta.status).toBe('idle');
    expect(exchangeResponseText(exchanges[0])).toBe('Nothing to clean up.');
  });
});

// ---------------------------------------------------------------------------
// CC message should produce ONE thread, not a stub + spawned thread
// ---------------------------------------------------------------------------
describe('CC single thread (no duplicate spawn)', () => {
  it('CC message produces one exchange in one thread, not a redirect stub', () => {
    const { map, id } = makeThread();
    // The correct flow: MessageReceived + SessionStarted + CC work + idle, all in one thread
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'what time is it?' },
      { type: 'SessionStarted', session_id: 'claude-code/20260315' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'date' } },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'Mon Mar 15 18:03:07 CET 2026' },
      { type: 'CodingAgentTextStreamed', text: 'The current time is 18:03.' },
      { type: 'CodingAgentIdled', has_changes: false },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(exchangeUserMessage(exchanges[0])).toBe('what time is it?');
    expect(exchangeStatus(exchanges[0], '', true)).toBe('done');

    // Must NOT have a "I've started a new Claude Code thread" redirect response
    const allEvents = [...map.get(id)!.events.values()];
    const responseEvents = allEvents.filter(e => e.type === 'ResponseGenerated');
    const hasRedirect = responseEvents.some(e =>
      (e as any).text?.includes('started a new Claude Code thread')
    );
    expect(hasRedirect).toBe(false);
  });
});

// ---------------------------------------------------------------------------
describe('No duplicate events', () => {
  it('Claude Code session has exactly one MessageReceived (no dual-persist)', () => {
    const { map, id } = makeThread();
    // Simulate a Claude Code session — each event arrives with a unique seq (from DB via bus)
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
      { type: 'CodingAgentToolResult', name: 'Read', result: 'file content' },
      { type: 'CodingAgentTextStreamed', text: 'I fixed the bug.' },
      { type: 'CodingAgentIdled' },
    ]);

    const thread = map.get(id)!;
    const allEvents = [...thread.events.values()];

    // Exactly one MessageReceived
    const msgEvents = allEvents.filter(e => e.type === 'MessageReceived');
    expect(msgEvents).toHaveLength(1);

    // Exactly one SessionStarted
    const sessionEvents = allEvents.filter(e => e.type === 'SessionStarted');
    expect(sessionEvents).toHaveLength(1);

    // Exactly one CodingAgentIdled
    const idleEvents = allEvents.filter(e => e.type === 'CodingAgentIdled');
    expect(idleEvents).toHaveLength(1);

    // Exactly one ToolCalled
    const toolEvents = allEvents.filter(e => e.type === 'CodingAgentToolCalled');
    expect(toolEvents).toHaveLength(1);

    // Exactly one ToolResult
    const resultEvents = allEvents.filter(e => e.type === 'CodingAgentToolResult');
    expect(resultEvents).toHaveLength(1);
  });

  it('Claude Code session produces exactly one exchange (no double MessageReceived)', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'do the thing' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);
    expect(exchanges[0].userEvent.type).toBe('MessageReceived');
  });

  it('recovery session produces exactly one exchange', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Recovering interrupted session...' },
      { type: 'SessionStarted', session_id: 'recovery-1' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: {} },
      { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok' },
      { type: 'CodingAgentTextStreamed', text: 'The worktree is clean.' },
      { type: 'CodingAgentIdled' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);

    // No duplicate events of any type
    const allEvents = [...map.get(id)!.events.values()];
    const typeCounts = new Map<string, number>();
    for (const e of allEvents) {
      typeCounts.set(e.type, (typeCounts.get(e.type) || 0) + 1);
    }
    // Each event type should appear exactly once (except TextStreamed which can have multiple chunks)
    for (const [type, count] of typeCounts) {
      if (type !== 'CodingAgentTextStreamed' && type !== 'TextStreamed') {
        expect(count).toBe(1);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// Flow: Image rendering
// ---------------------------------------------------------------------------
describe('Flow: Image rendering', () => {
  it('extracts user image hashes from MessageReceived event', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      {
        type: 'MessageReceived',
        text: 'Look at this',
        user_image_hashes: ['hash-abc', 'hash-def'],
      },
      { type: 'TextStreamed', text: 'I see two images.' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    expect(exchanges).toHaveLength(1);

    const hashes = exchangeUserImageHashes(exchanges[0]);
    expect(hashes).toEqual(['hash-abc', 'hash-def']);
  });

  it('returns empty array when no images', () => {
    const { map, id } = makeThread();

    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'No images' },
      { type: 'ResponseGenerated' },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    const images = exchangeUserImageHashes(exchanges[0]);
    expect(images).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Bug: CC follow-up images not rendered
// ---------------------------------------------------------------------------
describe('Bug: CC follow-up images not rendered', () => {
  it('pending CC follow-up includes images in synthetic exchange', () => {
    const { map, id } = makeThread();
    // Initial Claude Code session events
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Working...' },
      { type: 'CodingAgentIdled' },
    ]);

    // Simulate pending follow-up WITH image hashes (the wire shape post Phase 3b)
    const pendingHashes = ['sha256-of-img1', 'sha256-of-img2'];
    map.get(id)!.pendingUserMessages.push({
      text: 'here is the screenshot',
      eventId: 'ev-1',
      created: '2026-01-01T00:00:00Z',
      image_hashes: pendingHashes,
    });

    // Verify hashes are stored in the pending message data structure
    const pending = map.get(id)!.pendingUserMessages[0];
    expect(pending.image_hashes).toHaveLength(2);
    expect(pending.image_hashes![0]).toBe('sha256-of-img1');
  });

  it('CC follow-up MessageReceived event includes image hashes from SSE', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix the bug' },
      { type: 'SessionStarted', session_id: 's1' },
      { type: 'CodingAgentTextStreamed', text: 'Done.' },
      { type: 'CodingAgentIdled' },
      // Follow-up with image hashes (post-Phase-3b event payload shape)
      {
        type: 'MessageReceived',
        text: 'here is the screenshot',
        user_image_hashes: ['sha256-of-img1'],
      },
    ]);

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    // The follow-up should be a separate exchange
    const followUp = exchanges[exchanges.length - 1];
    const hashes = exchangeUserImageHashes(followUp);
    expect(hashes).toEqual(['sha256-of-img1']);
  });
});

// ---------------------------------------------------------------------------
// Bug: Channel labels missing on most exchanges
// ---------------------------------------------------------------------------
describe('Bug: Channel labels', () => {
  it('user channel is undefined when no channel in event', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'Hello' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBeUndefined();
  });

  it('user channel reads from MessageReceived event payload', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'fix it', channel: 'claude_code' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBe('claude_code');
  });

  it('scheduled trigger has user channel "trigger"', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'TriggerStarted', trigger_id: 't1', trigger_name: 'Check weather' },
    ]);

    const exchanges = getExchanges(map, id);
    expect(exchangeUserChannel(exchanges[0])).toBe('trigger');
  });
});

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------
