import { describe, it, expect } from 'vitest';
import copySource from '../rungSwitchCopy.ts?raw';
import { rungSwitchCopy } from '../rungSwitchCopy';

const BASE = { from: 'Opus 5.5 · medium', to: 'Haiku 5.5 · low', canHandOff: true, rebuild: 'yes' as const };

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
    expect(copy.handoffLabel).toBe('Hand off to a fresh session');
    expect(copy.handoffDetail).toBe('Starts fresh on Haiku 5.5 · low and carries on from where this one stands.');
    expect(copy.switchHereLabel).toBe('Switch this thread');
  });

  it('words the switch on its own when no fresh session is offered', () => {
    const without = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42, canHandOff: false });
    expect(without.switchLabel).toBe('Switch anyway');
    expect(without.message).toContain('Switching to Haiku 5.5 · low');
    expect(without.message).toContain('about $0.42');
    expect(without.message).not.toMatch(/fresh session/i);
  });

  it('says a switch only may reread the history when the cache may be cold', () => {
    const handoff = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42, rebuild: 'unknown' });
    expect(handoff.message).toBe("The thread's prompt cache on Opus 5.5 · medium may still be warm.");
    expect(handoff.rewrite).toBe('May read its whole history again, one time. It costs about $0.42 more.');
    const alone = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42, rebuild: 'unknown', canHandOff: false });
    expect(alone.message).toBe('Switching to Haiku 5.5 · low may make the thread read its whole history again, one time. It costs about $0.42 more.');
    const warm = rungSwitchCopy({ ...BASE, extraCostUsd: 0.42 });
    expect(warm.message).toBe("The thread's prompt cache is warm on Opus 5.5 · medium.");
    expect(warm.rewrite).not.toMatch(/may/i);
  });

  it('derives the figure from the engine, never a literal in the source', () => {
    expect(copySource).not.toMatch(/\$\d/);
  });
});
