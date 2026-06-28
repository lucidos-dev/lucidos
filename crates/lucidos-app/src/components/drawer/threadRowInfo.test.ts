import { describe, it, expect } from 'vitest';
import { threadContextName, threadInfoRows, draftRowTooltip, type TooltipRow } from './threadRowInfo';
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

describe('threadInfoRows', () => {
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
    const rows = threadInfoRows(base, 'idle');
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
    const rows = threadInfoRows({ ...base, codingAgentProposed: true } as ThreadMeta, 'idle');
    expect(byLabel(rows, 'Status')?.value).toBe('Changes ready');
    expect(byLabel(rows, 'Status')?.tone).toBe('changes');
  });

  it('marks a running thread with the running tone', () => {
    const rows = threadInfoRows({ ...base, messageCount: 1 } as ThreadMeta, 'running');
    expect(byLabel(rows, 'Exchanges')?.value).toBe('1');
    expect(byLabel(rows, 'Status')?.value).toBe('Running');
    expect(byLabel(rows, 'Status')?.tone).toBe('running');
  });

  it('reads "Waiting" when idle with active children (matches the status dot)', () => {
    const rows = threadInfoRows({ ...base, activeChildrenCount: 2 } as ThreadMeta, 'idle');
    expect(byLabel(rows, 'Status')?.value).toBe('Waiting');
    expect(byLabel(rows, 'Status')?.tone).toBe('waiting');
  });

  it('lets the thread\'s own running state win over active children', () => {
    const rows = threadInfoRows({ ...base, activeChildrenCount: 2 } as ThreadMeta, 'running');
    expect(byLabel(rows, 'Status')?.value).toBe('Running');
    expect(byLabel(rows, 'Status')?.tone).toBe('running');
  });

  it('reads "Waiting for you" when paused on a question', () => {
    const rows = threadInfoRows(base, 'waiting_for_user_answer');
    expect(byLabel(rows, 'Status')?.value).toBe('Waiting for you');
    expect(byLabel(rows, 'Status')?.tone).toBe('waiting');
  });
});

describe('draftRowTooltip', () => {
  const createdAt = new Date(Date.now() - 60_000).toISOString();
  const byLabel = (rows: TooltipRow[], label: string) =>
    rows.find((r) => r.label === label);

  it('emits Status / Context / Created rows with a Draft status', () => {
    const rows = draftRowTooltip('claude_code', { kind: 'external', repoId: 'r1' }, 'my-repo', createdAt);
    expect(rows.map((r) => r.label)).toEqual(['Status', 'Repository', 'Created']);
    expect(byLabel(rows, 'Status')?.value).toBe('Draft');
    expect(byLabel(rows, 'Status')?.tone).toBe('idle');
    expect(byLabel(rows, 'Repository')?.value).toBe('my-repo');
    expect(byLabel(rows, 'Created')?.value).toMatch(/ago$/);
  });

  it('names the app for an app-scope coding draft', () => {
    const rows = draftRowTooltip('claude_code', { kind: 'app', appId: 'notes' }, 'notes', createdAt);
    expect(byLabel(rows, 'App')?.value).toBe('notes');
  });

  it('is a Chat type for a plain (non-coding) draft', () => {
    const rows = draftRowTooltip('lucidos', { kind: 'lucidos' }, undefined, createdAt);
    expect(byLabel(rows, 'Type')?.value).toBe('Chat');
  });
});
