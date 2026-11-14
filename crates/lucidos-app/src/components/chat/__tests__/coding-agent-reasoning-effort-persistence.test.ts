/**
 * Tests for CC model/effort per-thread behavior.
 *
 * Model and reasoning effort are per-thread — they come from backend events
 * (CodingAgentSettingsChanged) or the active session, never from cross-thread cache.
 * New threads start with null (no label shown = defaults).
 */
import { describe, it, expect } from 'vitest';

interface CodingAgentCommandsResponse {
  control_commands: unknown[];
  builtin_commands: string[];
  skill_commands: string[];
  current_model: string | null;
  current_reasoning_effort: string | null;
  has_active_session: boolean;
}

// ---------------------------------------------------------------------------
// Mirror the loadCommands logic from CodingAgentControlMenu.tsx
// ---------------------------------------------------------------------------

interface State {
  pendingEffort: string | null;
  pendingModel: string | null;
  currentEffort: string | null;
  currentModel: string | null;
  hasActiveSession: boolean;
}

/** Simulate loadCommands() — always updates model/effort from backend response.
 *  Clears pending only when the live session has adopted that exact value, so
 *  a stale in-flight fetch can't wipe a pending pick set after the fetch went
 *  out. No cross-thread caching. */
function handleLoadCommands(state: State, res: CodingAgentCommandsResponse): State {
  const next = { ...state };
  next.hasActiveSession = res.has_active_session;
  if (res.has_active_session) {
    if (res.current_reasoning_effort === next.pendingEffort) {
      next.pendingEffort = null;
    }
    if (res.current_model === next.pendingModel) {
      next.pendingModel = null;
    }
  }
  next.currentEffort = (res.current_reasoning_effort as string) ?? null;
  next.currentModel = (res.current_model as string) ?? null;
  return next;
}

/** Mirror currentValueLabel() — pending overrides backend value. */
function currentValueLabel(state: State, field: 'effort' | 'model'): string | null {
  if (field === 'effort') return state.pendingEffort ?? state.currentEffort;
  return state.pendingModel ?? state.currentModel;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('CC model/effort: per-thread, no cross-thread cache', () => {
  // Fresh thread — no prior state
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('new thread starts with no model/effort labels', () => {
    expect(currentValueLabel(freshState, 'model')).toBeNull();
    expect(currentValueLabel(freshState, 'effort')).toBeNull();
  });

  it('backend response populates per-thread values', () => {
    const res: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'max',
      has_active_session: true,
    };
    const state = handleLoadCommands(freshState, res);
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
    expect(currentValueLabel(state, 'effort')).toBe('max');
  });

  it('null response clears stale values (thread switch)', () => {
    // Thread A had values
    const threadA: State = {
      ...freshState,
      currentModel: 'opus[1m]',
      currentEffort: 'max',
    };
    // Switch to thread B — backend returns null (no Claude Code session, no events)
    const res: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    };
    const state = handleLoadCommands(threadA, res);
    expect(currentValueLabel(state, 'model')).toBeNull();
    expect(currentValueLabel(state, 'effort')).toBeNull();
  });
});

describe('CC pending preferences survive until session confirms', () => {
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('pending values shown before session starts', () => {
    const state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });

  it('pending preserved through cache responses (no active session)', () => {
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    const cacheRes: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: null,
      current_reasoning_effort: null,
      has_active_session: false,
    };
    state = handleLoadCommands(state, cacheRes);
    state = handleLoadCommands(state, cacheRes);
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });

  it('pending cleared when session confirms with actual values', () => {
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'opus[1m]' };
    const sessionRes: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'max',
      has_active_session: true,
    };
    state = handleLoadCommands(state, sessionRes);
    expect(state.pendingEffort).toBeNull();
    expect(state.pendingModel).toBeNull();
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('opus[1m]');
  });

  it('pending preserved when active-session response carries a different value (stale fetch race)', () => {
    // Regression: cc-mid-session-settings.spec.ts on mobile-webkit. Reproduced
    // in isolation against a fresh workspace.
    // Sequence:
    //   1. First CC turn ran with effort='high' (chat default), session
    //      emitted CodingAgentSettingsChanged{high}; loadCommands fired and
    //      cached current_reasoning_effort='high'.
    //   2. The 1st-turn loadCommands fetch returned has_active_session=true
    //      from when the session was still in the agent_sessions map (before
    //      the idle-exit removed it). The response is in flight.
    //   3. CC idled, session removed. The user clicks the CC menu, picks
    //      Reasoning → Max, which sets pendingEffort='max'.
    //   4. The stale in-flight fetch from step 2 lands. Old logic cleared
    //      pendingEffort because has_active_session=true, even though the
    //      response's current_reasoning_effort='high' didn't match.
    //   5. sendMessage built the body with pendingEffort=null → fell back to
    //      the default 'high' → backend spawned the follow-up with 'high'.
    //   6. The test asserted current_reasoning_effort='max' → "Received: high".
    // Fix: only clear pending when the response's value actually matches the
    // pending pick, so a stale fetch can't wipe an intent set after it went
    // out.
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'haiku' };
    const staleRes: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'high',
      has_active_session: true,
    };
    state = handleLoadCommands(state, staleRes);
    expect(state.pendingEffort).toBe('max');
    expect(state.pendingModel).toBe('haiku');
    expect(currentValueLabel(state, 'effort')).toBe('max');
    expect(currentValueLabel(state, 'model')).toBe('haiku');
  });

  it('only the matching pending field clears when the response confirms one but not the other', () => {
    // pendingEffort matches but pendingModel doesn't — half-adopted. Wiping
    // both would lose the model pick the user still expects to take effect.
    let state: State = { ...freshState, pendingEffort: 'max', pendingModel: 'haiku' };
    const partialRes: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'opus[1m]',
      current_reasoning_effort: 'max',
      has_active_session: true,
    };
    state = handleLoadCommands(state, partialRes);
    expect(state.pendingEffort).toBeNull();
    expect(state.pendingModel).toBe('haiku');
  });
});

describe('CC model/effort from thread events (no active session)', () => {
  const freshState: State = {
    pendingEffort: null,
    pendingModel: null,
    currentEffort: null,
    currentModel: null,
    hasActiveSession: false,
  };

  it('existing thread with CodingAgentSettingsChanged events shows stored values', () => {
    // Backend returns values from CodingAgentSettingsChanged events for this thread
    const res: CodingAgentCommandsResponse = {
      control_commands: [],
      builtin_commands: ['help'],
      skill_commands: [],
      current_model: 'opus',
      current_reasoning_effort: 'high',
      has_active_session: false,
    };
    const state = handleLoadCommands(freshState, res);
    expect(currentValueLabel(state, 'model')).toBe('opus');
    expect(currentValueLabel(state, 'effort')).toBe('high');
  });

  it('values update when backend returns different thread settings', () => {
    // First load — thread A settings
    let state = handleLoadCommands(freshState, {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'opus',
      current_reasoning_effort: 'high',
      has_active_session: false,
    });
    // Switch to thread B — different settings from events
    state = handleLoadCommands(state, {
      control_commands: [],
      builtin_commands: [],
      skill_commands: [],
      current_model: 'sonnet',
      current_reasoning_effort: 'max',
      has_active_session: false,
    });
    expect(currentValueLabel(state, 'model')).toBe('sonnet');
    expect(currentValueLabel(state, 'effort')).toBe('max');
  });
});
