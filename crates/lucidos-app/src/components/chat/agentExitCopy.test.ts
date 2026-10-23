import { describe, expect, it } from 'vitest';
import { agentExitCopy } from './agentExitCopy';

describe('agentExitCopy', () => {
  it('says Claude Code could not start, with its exit code and why Lucidos did not retry', () => {
    expect(agentExitCopy({ stage: 'startup', code: 1 }, 'claude-code')).toEqual({
      heading: 'Claude Code could not start',
      reason: 'It exited with code 1 before its session began. Lucidos did not retry, because a start failure repeats until its cause is fixed.',
    });
  });

  it('names Codex when the thread runs Codex', () => {
    expect(agentExitCopy({ stage: 'startup', code: 2 }, 'codex').heading).toBe('Codex could not start');
  });

  it('leaves the code out when a signal ended the process', () => {
    expect(agentExitCopy({ stage: 'startup' }, 'claude-code').reason).toBe(
      'It exited before its session began. Lucidos did not retry, because a start failure repeats until its cause is fixed.',
    );
  });

  it('words a crash after the session began as a stop, not a start failure', () => {
    expect(agentExitCopy({ stage: 'mid_turn', code: 137 }, 'codex')).toEqual({
      heading: 'Codex stopped unexpectedly',
      reason: 'It exited with code 137 in the middle of a reply. Lucidos did not retry it.',
    });
    expect(agentExitCopy({ stage: 'mid_turn' }, 'claude-code').reason).toBe(
      'It exited in the middle of a reply. Lucidos did not retry it.',
    );
  });
});
