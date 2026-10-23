// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { HandoffSessionRow } from '../chat-exchange-parts';
import { drawsInitiatorPanel } from '../ChatExchange';
import { drawsResponseRow } from '../../../store/event-rendering';
import {
  applyAggregateToMeta, exchangeResponseEvents, makeOptimisticThreadState,
  type Exchange, type StoredEvent, type ThreadAggregate,
} from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

// A successor session starts in the same thread, behind one card: the card a
// fresh session gets, saying where the handoff ran and what the session runs
// on, with the opening folded under it.

const OPENING = '# Handoff\n\nPhases 0 to 3 are committed. Continue with phase 4.';

const HANDOFF = { kind: 'handoff', checkpoint: 'phase-commit', progress: '4/7', opening: 'narrative', message: OPENING };

function successorTurn(first: Record<string, unknown>): Exchange {
  const started = { type: 'SessionStarted', session_id: 's2', channel: 'claude_code', coding_agent: 'codex', continuity: HANDOFF };
  return {
    userEvent: { type: 'MessageReceived', channel: 'claude_code', ...first } as StoredEvent,
    userSeq: 0,
    steps: [
      { seq: 1, event: started as StoredEvent },
      { seq: 2, event: { type: 'CodingAgentTextStreamed', text: 'Picking up phase 4.' } as StoredEvent },
    ],
  };
}

const ENGINE_OPENING = {
  text: OPENING,
  model: 'gpt-6.1-sol',
  reasoning_effort: 'medium',
  origin: { kind: 'engine', reason: { kind: 'successor_session' } },
};

const cards = (exchange: Exchange) =>
  exchangeResponseEvents(exchange).filter((e): e is Extract<ResponseEvent, { type: 'handoff_session' }> => e.type === 'handoff_session');

describe('a successor session draws one card', () => {
  it('names the checkpoint, the backend and the model its first message ran on', () => {
    expect(cards(successorTurn(ENGINE_OPENING))).toMatchObject([{
      type: 'handoff_session',
      checkpoint: 'phase-commit',
      progress: '4/7',
      message: OPENING,
      coding_agent: 'codex',
      model: 'gpt-6.1-sol',
      reasoning_effort: 'medium',
    }]);
  });

  it('draws no fresh-session note beside it', () => {
    expect(exchangeResponseEvents(successorTurn(ENGINE_OPENING)).some((e) => e.type === 'fresh_session')).toBe(false);
  });

  it('draws no second copy of the engine message that opened it', () => {
    expect(drawsInitiatorPanel(successorTurn(ENGINE_OPENING))).toBe(false);
  });

  it('shows the engine message when no session card folds it, as after a failed spawn', () => {
    const failed = { ...successorTurn(ENGINE_OPENING), steps: [] };
    expect(drawsInitiatorPanel(failed)).toBe(true);
  });

  it('keeps your own message when it started the session, and still folds the opening', () => {
    const turn = successorTurn({ text: 'Also fix the typo.' });
    expect(drawsInitiatorPanel(turn)).toBe(true);
    expect(cards(turn)).toHaveLength(1);
    expect(cards(turn)[0].message).toBe(OPENING);
  });

  it('draws with the steps control off', () => {
    expect(drawsResponseRow(cards(successorTurn(ENGINE_OPENING))[0], false)).toBe(true);
  });
});

describe('the card reads as a fresh session card', () => {
  const card = (event: Extract<ResponseEvent, { type: 'handoff_session' }>) => JSON.stringify(HandoffSessionRow({ event }));

  it('says New session, Handed off, where it ran and what it runs on', () => {
    const s = card(cards(successorTurn(ENGINE_OPENING))[0]);
    expect(s).toContain('New session');
    expect(s).toContain('Handed off');
    expect(s).toContain('Carried on from a handoff at phase-commit 4/7.');
    expect(s).toContain('Codex');
    expect(s).toContain('What the new session was told');
  });

  it('names no checkpoint when the handoff ran at none', () => {
    const s = card({ type: 'handoff_session', message: OPENING });
    expect(s).toContain('Carried on from a handoff.');
  });
});

describe('the rung control reads the current session', () => {
  it('takes the backend a successor session started on', () => {
    const thread = makeOptimisticThreadState({
      id: 't1', title: 'Build it', channel: 'claude_code', initiator: 'user',
      eventsLoaded: true, timestamp: '2026-10-23T08:00:00.000Z', status: 'idle', codingAgent: 'claude-code',
    });
    const aggregate = {
      threadId: 't1', title: 'Build it', channel: 'claude_code', initiator: 'user',
      createdAt: '2026-10-23T08:00:00.000Z', lastActivity: '2026-10-23T08:00:00.000Z', messageCount: 2,
      section: 'inbox', status: 'running', summaryVersion: 2, activeChildrenCount: 0, totalChildrenCount: 0,
      blockingDescendantCount: 0, attentionDescendantCount: 0, liveEventWaitCount: 0,
      codingAgentChangeState: { kind: 'none' }, codingAgentIsExternalRepo: false, isSaved: false,
      hasResponse: true, lastRevivedAt: null, parentThreadId: null, parentThreadTitle: null, state: 'active',
      codingAgent: 'codex',
    } as ThreadAggregate;
    applyAggregateToMeta(thread.meta, aggregate);
    expect(thread.meta.codingAgent).toBe('codex');
  });
});
