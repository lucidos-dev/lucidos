import { describe, it, expect } from 'vitest';
import { threadContextName, threadRowTooltip, type TooltipRow } from './threadRowInfo';
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

  const byLabel = (rows: TooltipRow[], label: string) =>
    rows.find((r) => r.label === label);

  it('emits Status / You / Agent / Context / Exchanges / Started rows', () => {
    const rows = threadRowTooltip(base, 'idle');
    expect(rows.map((r) => r.label)).toEqual(['Status', 'You', 'Agent', 'Repository', 'Exchanges', 'Started']);
    expect(byLabel(rows, 'You')?.value).toMatch(/ago$/);
    expect(byLabel(rows, 'Agent')?.value).toMatch(/ago$/);
    expect(byLabel(rows, 'Repository')?.value).toBe('lucidos');
    expect(byLabel(rows, 'Status')?.value).toBe('Idle');
    expect(byLabel(rows, 'Status')?.tone).toBe('idle');
    expect(byLabel(rows, 'Exchanges')?.value).toBe('3');
    expect(byLabel(rows, 'Started')?.value).toMatch(/ago$/);
  });

  it('reads "Changes ready" (changes tone) when a change is proposed and not running', () => {
    const rows = threadRowTooltip({ ...base, codingAgentProposed: true } as ThreadMeta, 'idle');
    expect(byLabel(rows, 'Status')?.value).toBe('Changes ready');
    expect(byLabel(rows, 'Status')?.tone).toBe('changes');
  });

  it('marks a running thread with the running tone', () => {
    const rows = threadRowTooltip({ ...base, messageCount: 1 } as ThreadMeta, 'running');
    expect(byLabel(rows, 'Exchanges')?.value).toBe('1');
    expect(byLabel(rows, 'Status')?.value).toBe('Running');
    expect(byLabel(rows, 'Status')?.tone).toBe('running');
  });
});
