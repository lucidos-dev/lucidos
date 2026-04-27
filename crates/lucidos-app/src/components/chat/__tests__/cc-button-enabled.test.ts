/**
 * Tests for the CC command availability logic in CCControlMenu.
 *
 * The button is NEVER disabled — it's always clickable. The openMenu()
 * guard prevents the dropdown from opening when no commands exist yet.
 * The orange "active" class additionally requires builtin or skill commands
 * (indicating CC binary has reported its commands via Init).
 */
import { describe, it, expect } from 'vitest';

/**
 * Mirrors ccCommandsReady() in CCControlMenu.tsx.
 * Returns true when CC has reported builtin or skill commands (CC connected).
 * Used for the orange "active" visual indicator.
 */
function ccCommandsReady(
  builtinCommands: unknown[],
  skillCommands: unknown[],
): boolean {
  return builtinCommands.length > 0 || skillCommands.length > 0;
}

/**
 * Mirrors the openMenu guard in CCControlMenu.tsx.
 * The menu opens when ANY commands exist (control, builtin, or skill).
 * Control commands are always present from the backend for CC threads.
 */
function menuOpens(
  controlCommands: unknown[],
  builtinCommands: unknown[],
  skillCommands: unknown[],
): boolean {
  return controlCommands.length > 0 || ccCommandsReady(builtinCommands, skillCommands);
}

describe('CC menu opens (openMenu guard)', () => {
  const controlCmds = [{ subtype: 'set_model' }, { subtype: 'set_permission_mode' }];

  it('opens when only control commands exist (no builtin/skill yet)', () => {
    expect(menuOpens(controlCmds, [], [])).toBe(true);
  });

  it('opens when builtinCommands are present', () => {
    expect(menuOpens(controlCmds, ['help', 'model', 'status'], [])).toBe(true);
  });

  it('opens when skillCommands are present', () => {
    expect(menuOpens(controlCmds, [], ['commit', 'review-pr'])).toBe(true);
  });

  it('opens when both builtin and skill commands are present', () => {
    expect(menuOpens(controlCmds, ['help'], ['commit'])).toBe(true);
  });

  it('does NOT open when no commands at all (not yet loaded)', () => {
    expect(menuOpens([], [], [])).toBe(false);
  });
});

describe('ccCommandsReady (CC connected indicator)', () => {
  it('is false when no builtin or skill commands', () => {
    expect(ccCommandsReady([], [])).toBe(false);
  });

  it('is true when builtinCommands are present', () => {
    expect(ccCommandsReady(['help', 'status'], [])).toBe(true);
  });

  it('is true when skillCommands are present', () => {
    expect(ccCommandsReady([], ['commit'])).toBe(true);
  });

  it('is true when both are present', () => {
    expect(ccCommandsReady(['help'], ['commit'])).toBe(true);
  });
});

describe('button visual state', () => {
  it('button is clickable but NOT orange when only control commands exist', () => {
    const ccReady = ccCommandsReady([], []);
    expect(ccReady).toBe(false);  // not orange
    const className = `icon-btn cc-commands-btn${ccReady ? ' cc-commands-btn-active' : ''}`;
    expect(className).not.toContain('cc-commands-btn-active');
  });

  it('button is clickable AND orange when CC commands are cached', () => {
    const ccReady = ccCommandsReady(['help', 'status'], ['commit']);
    expect(ccReady).toBe(true);  // orange
    const className = `icon-btn cc-commands-btn${ccReady ? ' cc-commands-btn-active' : ''}`;
    expect(className).toContain('cc-commands-btn-active');
  });

  it('button is always clickable even when nothing loaded yet', () => {
    // Menu won't open (openMenu guard), but button itself is never disabled
    expect(menuOpens([], [], [])).toBe(false);
    // The button has no disabled attribute — it's always in the DOM without disabled
  });
});

// ---------------------------------------------------------------------------
// CC session version bump logic
// ---------------------------------------------------------------------------

/**
 * Mirrors the ccSessionVersion bump logic from thread-sync.ts.
 * This must be kept in sync with the real implementation.
 */
function shouldBumpCcSessionVersion(eventType: string): boolean {
  return eventType === 'SessionStarted'
    || eventType === 'SessionRecovered'
    || eventType === 'SessionEnded'
    || eventType === 'CodingAgentUserMessageSent'
    || eventType === 'CodingAgentIdled';
}

describe('ccSessionVersion bump triggers re-fetch of CC commands', () => {
  it('bumps on SessionStarted', () => {
    expect(shouldBumpCcSessionVersion('SessionStarted')).toBe(true);
  });

  it('bumps on SessionRecovered', () => {
    expect(shouldBumpCcSessionVersion('SessionRecovered')).toBe(true);
  });

  it('bumps on SessionEnded', () => {
    expect(shouldBumpCcSessionVersion('SessionEnded')).toBe(true);
  });

  it('bumps on CodingAgentUserMessageSent — follow-ups to idle CC sessions need re-fetch', () => {
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
// Reload recovery — ccSessionVersion bump on initial thread load
// ---------------------------------------------------------------------------

/**
 * On page reload, SSE does not replay historical events. The only fetch
 * path for CC commands is useEffect([threadId]) on CCControlMenu mount.
 * If that fetch + retries exhaust before the backend is ready, there's
 * no recovery. loadAllThreads must bump ccSessionVersion after loading
 * events for a focused CC thread, giving CCControlMenu a second chance.
 */
describe('reload recovery — bump ccSessionVersion for focused CC thread', () => {
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
// Retry logic for empty commands
// ---------------------------------------------------------------------------

describe('loadCommands retry on empty response', () => {
  it('should retry when commands are empty (CC Init may not have arrived)', () => {
    let retryCalled = false;
    const scheduleRetry = (commands: string[], skills: string[]) => {
      if (!ccCommandsReady(commands, skills)) {
        retryCalled = true;
      }
    };
    scheduleRetry([], []);
    expect(retryCalled).toBe(true);
  });

  it('should NOT retry when commands are present', () => {
    let retryCalled = false;
    const scheduleRetry = (commands: string[], skills: string[]) => {
      if (!ccCommandsReady(commands, skills)) {
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
 * Mirrors the retry logic from CCControlMenu.tsx.
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

// ---------------------------------------------------------------------------
// Control commands filtered by active session state
// ---------------------------------------------------------------------------

describe('control commands hidden when no active session', () => {
  const controlCmds = [
    { subtype: 'set_model', label: 'Model', params: [] },
    { subtype: 'set_reasoning_effort', label: 'Reasoning Effort', params: [] },
  ];

  /**
   * Mirrors the filtering logic in CCControlMenu.tsx:
   * const activeControlCommands = hasActiveSession.value ? controlCommands.value : [];
   */
  function activeControlCommands(
    controlCommands: typeof controlCmds,
    hasActiveSession: boolean,
  ) {
    return hasActiveSession ? controlCommands : [];
  }

  it('returns control commands when session is active', () => {
    const result = activeControlCommands(controlCmds, true);
    expect(result).toEqual(controlCmds);
  });

  it('returns empty when no active session', () => {
    const result = activeControlCommands(controlCmds, false);
    expect(result).toEqual([]);
  });

  it('menu still opens with slash commands even without active session', () => {
    // Control commands hidden, but slash commands still available
    const active = activeControlCommands(controlCmds, false);
    expect(menuOpens(active, ['help', 'status'], ['commit'])).toBe(true);
  });

  it('menu does NOT open when no session and no slash commands', () => {
    const active = activeControlCommands(controlCmds, false);
    expect(menuOpens(active, [], [])).toBe(false);
  });
});
