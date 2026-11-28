import { describe, it, expect } from 'vitest';
import { aggregatePongAnswers, PONG_COLLECT_MS } from './protocol';
import type { PongAnswer } from '../eventStream';

function answer(over: Partial<PongAnswer> = {}): PongAnswer {
  return { device_id: 'dev-1', outcome: 'not_shown', ...over };
}

describe('aggregatePongAnswers', () => {
  it('returns null when nothing answered, so there is no pong to POST', () => {
    expect(aggregatePongAnswers([])).toBeNull();
  });

  it('passes a lone answer through unchanged', () => {
    const only = answer({ outcome: 'auto_read' });
    expect(aggregatePongAnswers([only])).toEqual(only);
  });

  it('lets one document that showed the toast speak for the connection', () => {
    const merged = aggregatePongAnswers([
      answer({ outcome: 'not_shown' }),
      answer({ outcome: 'shown' }),
      answer({ outcome: 'not_shown' }),
    ]);
    expect(merged?.outcome).toBe('shown');
  });

  it('keeps the strongest of the handled outcomes', () => {
    expect(aggregatePongAnswers([answer({ outcome: 'silenced' }), answer({ outcome: 'auto_read' })])?.outcome)
      .toBe('auto_read');
    expect(aggregatePongAnswers([answer({ outcome: 'auto_read' }), answer({ outcome: 'shown' })])?.outcome)
      .toBe('shown');
  });

  it('reports not_shown only when every document is', () => {
    const merged = aggregatePongAnswers([answer(), answer(), answer()]);
    expect(merged?.outcome).toBe('not_shown');
  });

  it('carries the device id, which every document of a workspace shares', () => {
    const merged = aggregatePongAnswers([answer({ device_id: 'dev-9' }), answer({ device_id: 'dev-9' })]);
    expect(merged?.device_id).toBe('dev-9');
  });
});

describe('PONG_COLLECT_MS', () => {
  it('stays well inside the engine deadline it must beat', () => {
    const ENGINE_DEADLINE_MS = 2000;
    expect(PONG_COLLECT_MS).toBeLessThan(ENGINE_DEADLINE_MS / 2);
  });
});
