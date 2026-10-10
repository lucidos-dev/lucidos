import { describe, it, expect, beforeEach } from 'vitest';
import {
  addMarkdownParseMs,
  addLinkifyMs,
  readRenderPhaseTotals,
  currentPerfBaseline,
  _resetRenderPhaseTimersForTesting,
} from './renderPhaseTimers';

describe('renderPhaseTimers', () => {
  beforeEach(() => _resetRenderPhaseTimersForTesting());

  it('accumulates markdown and linkify time independently', () => {
    addMarkdownParseMs(10);
    addMarkdownParseMs(5);
    addLinkifyMs(3);
    expect(readRenderPhaseTotals()).toEqual({ markdownMs: 15, linkifyMs: 3 });
  });

  it('starts at zero after reset', () => {
    expect(readRenderPhaseTotals()).toEqual({ markdownMs: 0, linkifyMs: 0 });
  });

  it('per-operation cost is the delta against a baseline', () => {
    addMarkdownParseMs(100); // unrelated prior work
    const base = currentPerfBaseline();
    addMarkdownParseMs(7);
    addLinkifyMs(2);
    const now = readRenderPhaseTotals();
    expect(now.markdownMs - base.md).toBe(7);
    expect(now.linkifyMs - base.link).toBe(2);
  });

  it('currentPerfBaseline captures a start timestamp and current totals', () => {
    addMarkdownParseMs(4);
    addLinkifyMs(9);
    const base = currentPerfBaseline();
    expect(base.md).toBe(4);
    expect(base.link).toBe(9);
    expect(typeof base.start).toBe('number');
  });
});
