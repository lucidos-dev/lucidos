import { describe, it, expect } from 'vitest';
import copySource from '../rungSwitchCopy.ts?raw';
import { rungSwitchCopy } from '../rungSwitchCopy';

const BASE = { from: 'Opus 5.5 · medium', to: 'Haiku 5.5 · low', canHandOff: true };

describe('the rung switch confirm copy', () => {
  it('names the one-time re-read, with the engine figure, and never a lasting slowdown', () => {
    const { rewrite } = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42 });
    expect(rewrite).toMatch(/one time/i);
    expect(rewrite).toContain('about $0.42');
    expect(rewrite).not.toMatch(/slow/i);
  });

  it('states no figure when the engine has none', () => {
    const { rewrite } = rungSwitchCopy({ ...BASE, extraCostUsd: null });
    expect(rewrite).toMatch(/one time/i);
    expect(rewrite).not.toContain('$');
  });

  it('calls a figure under a cent small, instead of printing $0.00', () => {
    const { rewrite } = rungSwitchCopy({ ...BASE, extraCostUsd: 0.002 });
    expect(rewrite).toContain('less than $0.01');
    expect(rewrite).not.toContain('$0.00');
  });

  it('names the target rung in the title and the current one on Keep', () => {
    const copy = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42 });
    expect(copy.title).toBe('Switch to Haiku 5.5 · low');
    expect(copy.keepLabel).toBe('Keep Opus 5.5 · medium');
    expect(copy.handoffLabel).toBe('Hand off to a new thread');
    expect(copy.handoffDetail).toBe('Starts fresh on Haiku 5.5 · low and carries on from where this one stands.');
    expect(copy.switchHereLabel).toBe('Switch this thread');
  });

  it('words the switch on its own when no new thread is offered', () => {
    const without = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42, canHandOff: false });
    expect(without.switchLabel).toBe('Switch anyway');
    expect(without.message).toContain('Switching to Haiku 5.5 · low');
    expect(without.message).toContain('about $0.42');
    expect(without.message).not.toMatch(/new thread/i);
  });

  it('derives the figure from the engine, never a literal in the source', () => {
    expect(copySource).not.toMatch(/\$\d/);
  });
});
