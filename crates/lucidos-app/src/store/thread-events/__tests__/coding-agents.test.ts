import { describe, expect, it } from 'vitest';
import { applyAggregateToMeta, type ThreadAggregate, type ThreadMeta } from '../thread-meta';

/** Only the fields `applyAggregateToMeta` reads before it reaches the backend
 *  set; a full fixture would just rot. */
function meta(codingAgents?: ThreadMeta['codingAgents']): ThreadMeta {
  return {
    status: 'idle',
    summaryVersion: 1,
    liveEventWaitCount: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    saved: false,
    messageCount: 0,
    codingAgents,
  } as unknown as ThreadMeta;
}

function aggregate(codingAgents: ThreadAggregate['codingAgents'], summaryVersion: number): ThreadAggregate {
  return {
    summaryVersion,
    status: 'idle',
    liveEventWaitCount: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    isSaved: false,
    codingAgents,
  } as unknown as ThreadAggregate;
}

describe('the backends a thread used reach the row live', () => {
  it('a successor session on another backend adds its mark without a reload', () => {
    const m = meta(['claude-code']);
    expect(applyAggregateToMeta(m, aggregate(['claude-code', 'codex'], 2))).toBe(true);
    expect(m.codingAgents).toEqual(['claude-code', 'codex']);
  });

  it('an unchanged list is no change, so the drawer does not redraw', () => {
    const m = meta(['claude-code', 'codex']);
    applyAggregateToMeta(m, aggregate(undefined, 2));
    expect(applyAggregateToMeta(m, aggregate(['claude-code', 'codex'], 3))).toBe(false);
  });

  it('the first-use order is part of the value', () => {
    const m = meta(['claude-code', 'codex']);
    expect(applyAggregateToMeta(m, aggregate(['codex', 'claude-code'], 2))).toBe(true);
    expect(m.codingAgents).toEqual(['codex', 'claude-code']);
  });

  it('an aggregate that omits the list leaves it alone', () => {
    const m = meta(['claude-code', 'codex']);
    applyAggregateToMeta(m, aggregate(undefined, 2));
    expect(m.codingAgents).toEqual(['claude-code', 'codex']);
  });
});
