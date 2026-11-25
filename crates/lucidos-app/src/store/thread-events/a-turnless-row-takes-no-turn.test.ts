import { describe, expect, it } from 'vitest';
import { exchangeStatus, groupIntoExchanges, queuedFollowupRun, type StoredEvent } from '../thread-events';

const at = (second: number) => `2026-10-03T14:00:${String(second).padStart(2, '0')}.000Z`;

function answeredCardThenLateKeepAliveStatus(): Map<number, StoredEvent> {
  const events = new Map<number, StoredEvent>();
  events.set(1, { type: 'UserQuestionAsked', tool_use_id: 'q1', questions: [], created: at(0), _eventId: 'c1', request_event_id: 'r1', channel: 'claude_code' } as unknown as StoredEvent);
  events.set(2, { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'Selected', option_id: 'opt-0' }, created: at(3), channel: 'claude_code' } as unknown as StoredEvent);
  events.set(3, { type: 'CodingAgentPromptSent', text: 'answered', created: at(3), request_event_id: 'r1', channel: 'claude_code' } as StoredEvent);
  events.set(4, {
    type: 'CacheKeepAliveStatusChanged', created: at(6),
    status: {
      state: 'warming', since: at(0), beats: 0, budget_beats: 6, beat_every_secs: 230, cost_usd: 0, context_tokens: 1,
      give_up_at: at(59), ping_usd: 0.05, reread_usd: 1.3,
    },
  } as unknown as StoredEvent);
  events.set(5, { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'git log -1' }, created: at(9), request_event_id: 'r1', channel: 'claude_code' } as unknown as StoredEvent);
  return events;
}

describe('a turnless row arriving mid-turn', () => {
  it('leaves the running turn the live one, so it reads Working, not Done', () => {
    const exchanges = groupIntoExchanges(answeredCardThenLateKeepAliveStatus());
    const turn = exchanges.findIndex((ex) => ex.userEvent.type === 'UserQuestionAsked');
    for (const threadIsCC of [true, false]) {
      expect(queuedFollowupRun(exchanges, true, threadIsCC).activeIndex).toBe(turn);
    }
    const { activeIndex } = queuedFollowupRun(exchanges, true, true);
    expect(exchangeStatus(exchanges[turn], '', activeIndex === turn, false, true)).toBe('coding-agent-working');
  });
});
