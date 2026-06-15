import { describe, it, expect } from 'vitest';
import { threadContextName, threadRowTooltip } from './threadRowInfo';
import type { ThreadMeta } from '../../store/thread-events';

describe('threadContextName', () => {
  it('is the repo name for a coding-agent thread', () => {
    expect(threadContextName({ channel: 'claude_code', repoName: 'lucidos' })).toBe('lucidos');
  });
  it('is the app id for an app thread', () => {
    expect(threadContextName({
      channel: 'claude_code', codingAgentKind: 'app', codingAgentFolder: '/ws/data/apps/notes',
    })).toBe('notes');
  });
  it('is the trigger name for a trigger thread', () => {
    expect(threadContextName({ channel: 'trigger', triggerName: 'Nightly Build' })).toBe('Nightly Build');
  });
  it('is undefined for plain chat (the type tag already says it)', () => {
    expect(threadContextName({ channel: 'chat' })).toBeUndefined();
  });
});

describe('threadRowTooltip', () => {
  const base = {
    channel: 'claude_code',
    repoName: 'lucidos',
    lastUserAction: new Date(Date.now() - 2 * 3600_000).toISOString(),
    lastAgentAction: new Date(Date.now() - 60_000).toISOString(),
    createdAt: new Date(Date.now() - 3 * 86_400_000).toISOString(),
    updatedAt: new Date(Date.now() - 60_000).toISOString(),
    messageCount: 3,
    codingAgentProposed: false,
  } as unknown as ThreadMeta;

  it('includes You / Agent / Context / Status / exchanges / Started lines', () => {
    const tip = threadRowTooltip(base, 'idle');
    const lines = tip.split('\n');
    expect(lines[0]).toMatch(/^You · .+ago$/);
    expect(lines[1]).toMatch(/^Agent · .+ago$/);
    expect(tip).toContain('Repository · lucidos');
    expect(tip).toContain('Status · Idle');
    expect(tip).toContain('3 exchanges');
    expect(tip).toMatch(/Started · /);
  });

  it('reads "Changes ready" when a change is proposed and not running', () => {
    const tip = threadRowTooltip({ ...base, codingAgentProposed: true } as ThreadMeta, 'idle');
    expect(tip).toContain('Status · Changes ready');
  });

  it('singularizes a one-message thread', () => {
    const tip = threadRowTooltip({ ...base, messageCount: 1 } as ThreadMeta, 'running');
    expect(tip).toContain('1 exchange\n');
    expect(tip).toContain('Status · Running');
  });
});
