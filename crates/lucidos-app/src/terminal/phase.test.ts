import { describe, it, expect } from 'vitest';
import { exitCauseText, terminalStatusText } from './phase';

const context = { where: 'Worktree', attachments: 2, mobile: false };

describe('the terminal status line', () => {
  it('names where the shell runs and how many devices watch it', () => {
    const live = { kind: 'live', state: { kind: 'attached' } } as const;
    expect(terminalStatusText(live, context)).toBe('Running in worktree · 2 devices attached');
    expect(terminalStatusText(live, { ...context, mobile: true, attachments: 1 })).toBe('Worktree · 1 device');
  });

  it('says nothing while it is still finding out', () => {
    expect(terminalStatusText({ kind: 'checking' }, context)).toBe('');
  });

  it('says when the switch is off', () => {
    expect(terminalStatusText({ kind: 'off' }, context)).toBe('Terminal is off.');
  });

  it('says what to do when the relay blocks the live connection', () => {
    expect(terminalStatusText({ kind: 'relay-blocked', gateway: 'peer' }, context))
      .toContain('Open this workspace on "peer" directly');
  });

  it('gives a reason for every way a shell ends', () => {
    expect(exitCauseText('archived', null)).toBe('The thread was archived.');
    expect(exitCauseText('exited', 0)).toBe('The shell exited.');
    expect(exitCauseText('exited', 2)).toBe('The shell exited with status 2.');
  });
});
