/**
 * Pong aggregation is what keeps the engine's `expected_pong_count` honest once
 * many documents sit behind one connection. The engine waits for one pong per
 * open SSE stream, so the worker owes it exactly one, ORed across its ports.
 */
import { describe, it, expect } from 'vitest';
import { aggregatePongAnswers, PONG_COLLECT_MS } from './protocol';
import type { PongAnswer } from '../eventStream';

function answer(over: Partial<PongAnswer> = {}): PongAnswer {
  return {
    device_id: 'dev-1',
    is_active: false,
    focused_thread_id: null,
    event_in_viewport: false,
    ...over,
  };
}

describe('aggregatePongAnswers', () => {
  it('returns null when nothing answered, so there is no pong to POST', () => {
    expect(aggregatePongAnswers([])).toBeNull();
  });

  it('passes a lone answer through unchanged', () => {
    const only = answer({ is_active: true, focused_thread_id: 't-1' });
    expect(aggregatePongAnswers([only])).toEqual(only);
  });

  it('ORs is_active, so one visible tab keeps the device active', () => {
    const merged = aggregatePongAnswers([
      answer({ is_active: false }),
      answer({ is_active: true }),
      answer({ is_active: false }),
    ]);
    expect(merged?.is_active).toBe(true);
  });

  it('reports inactive only when every document is', () => {
    const merged = aggregatePongAnswers([answer(), answer(), answer()]);
    expect(merged?.is_active).toBe(false);
  });

  it('ORs event_in_viewport, so one tab reading the event auto-reads it', () => {
    const merged = aggregatePongAnswers([
      answer({ event_in_viewport: false }),
      answer({ event_in_viewport: true }),
    ]);
    expect(merged?.event_in_viewport).toBe(true);
  });

  it('prefers an ACTIVE document\'s focused thread over a hidden one\'s', () => {
    // The engine reads focused_thread_id to decide whether the user is looking
    // at the source thread. A hidden tab's answer says nothing about that, so
    // it must not win over a visible tab's.
    const merged = aggregatePongAnswers([
      answer({ is_active: false, focused_thread_id: 'hidden-thread' }),
      answer({ is_active: true, focused_thread_id: 'visible-thread' }),
    ]);
    expect(merged?.focused_thread_id).toBe('visible-thread');
  });

  it('falls back to a hidden document\'s focused thread when none is active', () => {
    const merged = aggregatePongAnswers([
      answer({ is_active: false, focused_thread_id: null }),
      answer({ is_active: false, focused_thread_id: 'hidden-thread' }),
    ]);
    expect(merged?.focused_thread_id).toBe('hidden-thread');
  });

  it('carries the device id, which every document of a workspace shares', () => {
    const merged = aggregatePongAnswers([answer({ device_id: 'dev-9' }), answer({ device_id: 'dev-9' })]);
    expect(merged?.device_id).toBe('dev-9');
  });
});

describe('PONG_COLLECT_MS', () => {
  it('stays well inside the engine deadline it must beat', () => {
    // `DEADLINE_MS` in crates/lucidos-engine/src/scheduler/push.rs. An
    // aggregated pong arriving after it informs no decision at all.
    const ENGINE_DEADLINE_MS = 2000;
    expect(PONG_COLLECT_MS).toBeLessThan(ENGINE_DEADLINE_MS / 2);
  });
});
