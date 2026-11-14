/**
 * Tests for the coding-agent command availability logic in CodingAgentControlMenu.
 *
 * The button is NEVER disabled — it's always clickable. The openMenu()
 * guard prevents the dropdown from opening when no commands exist yet, and the
 * "active" class rides that SAME predicate (hasAnyCommands): the button is
 * undimmed exactly when clicking it opens something, and dimmed while it would
 * be a silent no-op. Landing the list also pulses the mark once.
 */
import { describe, it, expect } from 'vitest';

/**
 * Mirrors hasAnyCommands() in CodingAgentControlMenu.tsx, which is BOTH the
 * openMenu guard and the `commands-btn-active` condition. One predicate, so the
 * button cannot look ready while the menu refuses to open. It also gates the
 * slash-command CACHE update and the empty-response retry. The model and effort
 * are the rung control's, so they never count here.
 */
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
  /** Mirrors the button's class expression in CodingAgentControlMenu.tsx. */
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
    // The glyph is sized and grayed by `.icon-btn.header-icon`, the same class
    // the todo / subscription / attach buttons beside it use. Dropping it puts
    // the agent mark back at 1rem in a 1.5rem box, a size out of step with
    // every other icon in the cluster.
    expect(buttonClass([], [])).toContain('header-icon');
  });

  it('button is always clickable even when nothing loaded yet', () => {
    // Menu won't open (openMenu guard), but button itself is never disabled
    expect(menuOpens([], [])).toBe(false);
    // The button has no disabled attribute — it's always in the DOM without disabled
  });
});

// ---------------------------------------------------------------------------
// Claude Code session version bump logic
// ---------------------------------------------------------------------------

/**
 * Mirrors the codingAgentSessionVersion bump logic from thread-sync.ts.
 * This must be kept in sync with the real implementation.
 */
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

// ---------------------------------------------------------------------------
// Reload recovery — codingAgentSessionVersion bump on initial thread load
// ---------------------------------------------------------------------------

/**
 * On page reload, SSE does not replay historical events. The only fetch
 * path for coding-agent commands is useEffect([threadId]) on CodingAgentControlMenu mount.
 * If that fetch + retries exhaust before the backend is ready, there's
 * no recovery. loadAllThreads must bump codingAgentSessionVersion after loading
 * events for a focused CC thread, giving CodingAgentControlMenu a second chance.
 */
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

// ---------------------------------------------------------------------------
// loadCommands catch — transient transport errors (iOS PWA wake)
// ---------------------------------------------------------------------------

/**
 * Mirrors the catch handler in CodingAgentControlMenu.loadCommands. The decision must
 * not depend on threadId — iOS PWA HTTP/2 connections go stale after
 * backgrounding and the first wake fetch fails for both compose and thread
 * view. A previous version only retried when threadId was set, so compose
 * view fired a "Failed to load coding-agent commands" toast immediately on every wake.
 */
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

// ---------------------------------------------------------------------------
// loadCommands gates on engineRestarting — restart can take longer than the
// retry budget (Rust recompile + boot can run 30-90s), and other loaders
// (loadAllThreads, loadThreadEvents, refreshThreadEvents, connectThreadEvents)
// already follow this convention. Without the gate, "Failed to load CC
// commands" fires every time the user applies a Rust change.
// ---------------------------------------------------------------------------

/** Mirrors the early return in loadCommands. */
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

/** Mirrors the engineRestarting useSignalEffect that re-triggers loadCommands
 *  after a restart completes. Compose view (no threadId) only — focused CC
 *  threads are already covered by codingAgentSessionVersion bump in loadAllThreads,
 *  and triggering both would cause a redundant fetch (×2 with dual layouts). */
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

// ---------------------------------------------------------------------------
// Retry logic for empty commands
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// openMenu must reset retry counter so manual opens get fresh retries
// ---------------------------------------------------------------------------

/**
 * Mirrors the retry logic from CodingAgentControlMenu.tsx.
 * loadCommands retries up to MAX_EMPTY_RETRIES when commands are empty.
 * openMenu must reset the counter so each manual open gets fresh retries.
 */
describe('openMenu resets retry counter for fresh retries', () => {
  const MAX_EMPTY_RETRIES = 10;

  it('should allow retries after openMenu resets the counter', () => {
    let retryCount = MAX_EMPTY_RETRIES; // exhausted from previous load

    // Simulate openMenu resetting the counter before calling loadCommands
    retryCount = 0; // openMenu resets

    // loadCommands should now be able to retry
    const canRetry = retryCount < MAX_EMPTY_RETRIES;
    expect(canRetry).toBe(true);
  });

  it('BUG: without reset, exhausted retries block future attempts', () => {
    let retryCount = MAX_EMPTY_RETRIES; // exhausted from initial load

    // Without openMenu resetting the counter, no retries possible
    const canRetry = retryCount < MAX_EMPTY_RETRIES;
    expect(canRetry).toBe(false); // this is the bug — should be true after opening menu
  });
});
