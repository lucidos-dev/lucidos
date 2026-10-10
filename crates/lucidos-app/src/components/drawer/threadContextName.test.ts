import { describe, it, expect } from 'vitest';
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
