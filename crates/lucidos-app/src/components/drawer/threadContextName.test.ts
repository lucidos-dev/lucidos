import { describe, it, expect } from 'vitest';
import { appsList } from '../../store/store';
import { threadContextName } from './threadContextName';

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

it('uses live app metadata after renames and keeps the id when unavailable', () => {
  const fields = { channel: 'claude_code', codingAgentKind: 'app' as const, codingAgentFolder: '/workspace/data/apps/task-board' };
  const app = { id: 'task-board', name: 'Task Board', description: '', reveal: 'on-load' as const, kind: 'app' as const, reusable: false };
  appsList.value = { status: 'loaded', data: [app] };
  expect(threadContextName(fields)).toBe('Task Board');
  appsList.value = { status: 'loaded', data: [{ ...app, name: 'Planning Board' }] };
  expect(threadContextName(fields)).toBe('Planning Board');
  appsList.value = { status: 'loaded', data: [] };
  expect(threadContextName(fields)).toBe('task-board');
  appsList.value = { status: 'failed', error: 'Unavailable' };
  expect(threadContextName(fields)).toBe('task-board');
  appsList.value = { status: 'not-loaded' };
});
