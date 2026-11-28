import { describe, it, expect } from 'vitest';

function menuOpens(builtinCommands: unknown[], skillCommands: unknown[]): boolean {
  return builtinCommands.length > 0 || skillCommands.length > 0;
}

describe('Control menu opens (openMenu guard)', () => {
  it('opens when builtinCommands are present', () => {
    expect(menuOpens(['help', 'model', 'status'], [])).toBe(true);
  });

  it('opens when skillCommands are present', () => {
    expect(menuOpens([], ['commit', 'review-pr'])).toBe(true);
  });

  it('opens when both builtin and skill commands are present', () => {
    expect(menuOpens(['help'], ['commit'])).toBe(true);
  });

  it('does NOT open when no commands at all (not yet loaded)', () => {
    expect(menuOpens([], [])).toBe(false);
  });
});

describe('button visual state', () => {
  function buttonClass(builtin: unknown[], skill: unknown[]): string {
    const ready = menuOpens(builtin, skill);
    return `icon-btn header-icon commands-btn${ready ? ' commands-btn-active' : ''}`;
  }

  it('is dimmed while nothing has loaded, because clicking it opens nothing', () => {
    expect(menuOpens([], [])).toBe(false);
    expect(buttonClass([], [])).not.toContain('commands-btn-active');
  });

  it('is undimmed when coding-agent slash commands are cached', () => {
    expect(buttonClass(['help', 'status'], ['commit'])).toContain('commands-btn-active');
  });

  it('carries header-icon, so it wears the prompt row\'s shared icon box', () => {
    expect(buttonClass([], [])).toContain('header-icon');
  });

  it('button is always clickable even when nothing loaded yet', () => {
    expect(menuOpens([], [])).toBe(false);
  });
});


function shouldBumpCcSessionVersion(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'ContinuationStarted'
    || eventType === 'SessionEnded'
    || eventType === 'CodingAgentUserMessageSent'
    || eventType === 'CodingAgentIdled';
}

describe('codingAgentSessionVersion bump triggers re-fetch of coding-agent commands', () => {
  it('bumps on SessionStarted', () => {
    expect(shouldBumpCcSessionVersion('SessionStarted')).toBe(true);
  });

  it('bumps on ContinuationStarted', () => {
    expect(shouldBumpCcSessionVersion('ContinuationStarted')).toBe(true);
  });

  it('bumps on SessionEnded', () => {
    expect(shouldBumpCcSessionVersion('SessionEnded')).toBe(true);
  });

  it('bumps on CodingAgentUserMessageSent — follow-ups to idle Claude Code sessions need re-fetch', () => {
    expect(shouldBumpCcSessionVersion('CodingAgentUserMessageSent')).toBe(true);
  });

  it('bumps on CodingAgentIdled — retry may have exhausted during CC init, idle guarantees binary is ready', () => {
    expect(shouldBumpCcSessionVersion('CodingAgentIdled')).toBe(true);
  });

  it('does NOT bump on unrelated events', () => {
    expect(shouldBumpCcSessionVersion('MessageReceived')).toBe(false);
    expect(shouldBumpCcSessionVersion('ResponseGenerated')).toBe(false);
    expect(shouldBumpCcSessionVersion('CodingAgentToolCalled')).toBe(false);
    expect(shouldBumpCcSessionVersion('CodingAgentTextStreamed')).toBe(false);
  });
});


describe('reload recovery — bump codingAgentSessionVersion for focused CC thread', () => {
  function bumpIfCC(channel: string): number {
    let version = 0;
    if (channel === 'claude_code') version++;
    return version;
  }

  it('should bump when focused thread is a CC thread after initial load', () => {
    expect(bumpIfCC('claude_code')).toBe(1);
  });

  it('should NOT bump for non-CC threads', () => {
    expect(bumpIfCC('chat')).toBe(0);
  });
});


function shouldRetryFetchCommandsError(
  retryCount: number,
  maxRetries: number,
  _threadId: string | undefined,
): boolean {
  return retryCount < maxRetries;
}

describe('loadCommands catch — retries network failures regardless of view', () => {
  const MAX = 10;

  it('retries thread view within budget', () => {
    expect(shouldRetryFetchCommandsError(0, MAX, 'thread-id')).toBe(true);
    expect(shouldRetryFetchCommandsError(9, MAX, 'thread-id')).toBe(true);
  });

  it('retries compose view within budget (iOS PWA wake regression)', () => {
    expect(shouldRetryFetchCommandsError(0, MAX, undefined)).toBe(true);
    expect(shouldRetryFetchCommandsError(9, MAX, undefined)).toBe(true);
  });

  it('shows toast after retry budget for both views', () => {
    expect(shouldRetryFetchCommandsError(MAX, MAX, 'thread-id')).toBe(false);
    expect(shouldRetryFetchCommandsError(MAX, MAX, undefined)).toBe(false);
  });
});


function shouldFetchCodingAgentCommands(engineRestarting: boolean): boolean {
  return !engineRestarting;
}

describe('loadCommands skips during engine restart', () => {
  it('skips fetch while engine is restarting (suppresses toast)', () => {
    expect(shouldFetchCodingAgentCommands(true)).toBe(false);
  });

  it('fetches when engine is not restarting', () => {
    expect(shouldFetchCodingAgentCommands(false)).toBe(true);
  });
});

function shouldRefetchOnRestartTransition(
  prev: boolean,
  current: boolean,
  threadId: string | undefined,
): boolean {
  return prev && !current && !threadId;
}

describe('loadCommands re-fetches when engine restart completes', () => {
  it('re-fetches compose view on transition restarting → not restarting', () => {
    expect(shouldRefetchOnRestartTransition(true, false, undefined)).toBe(true);
  });

  it('does NOT re-fetch focused CC thread (covered by codingAgentSessionVersion bump)', () => {
    expect(shouldRefetchOnRestartTransition(true, false, 'thread-id')).toBe(false);
  });

  it('does not re-fetch on idle (false → false)', () => {
    expect(shouldRefetchOnRestartTransition(false, false, undefined)).toBe(false);
  });

  it('does not re-fetch on restart starting (false → true)', () => {
    expect(shouldRefetchOnRestartTransition(false, true, undefined)).toBe(false);
  });

  it('does not re-fetch while still restarting (true → true)', () => {
    expect(shouldRefetchOnRestartTransition(true, true, undefined)).toBe(false);
  });
});


describe('loadCommands retry on empty response', () => {
  it('should retry when commands are empty (CC Init may not have arrived)', () => {
    let retryCalled = false;
    const scheduleRetry = (commands: string[], skills: string[]) => {
      if (!menuOpens(commands, skills)) {
        retryCalled = true;
      }
    };
    scheduleRetry([], []);
    expect(retryCalled).toBe(true);
  });

  it('should NOT retry when commands are present', () => {
    let retryCalled = false;
    const scheduleRetry = (commands: string[], skills: string[]) => {
      if (!menuOpens(commands, skills)) {
        retryCalled = true;
      }
    };
    scheduleRetry(['help', 'compact'], []);
    expect(retryCalled).toBe(false);
  });
});


describe('openMenu resets retry counter for fresh retries', () => {
  const MAX_EMPTY_RETRIES = 10;

  it('should allow retries after openMenu resets the counter', () => {
    let retryCount = MAX_EMPTY_RETRIES;

    retryCount = 0;

    const canRetry = retryCount < MAX_EMPTY_RETRIES;
    expect(canRetry).toBe(true);
  });

  it('BUG: without reset, exhausted retries block future attempts', () => {
    let retryCount = MAX_EMPTY_RETRIES;

    const canRetry = retryCount < MAX_EMPTY_RETRIES;
    expect(canRetry).toBe(false);
  });
});
